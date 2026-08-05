use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use fs2::FileExt;
use pk_core::WikiEntry;
use pk_store::MarkdownStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Parser)]
#[command(name = "prometheus-learning-worker", version)]
struct Cli {
    #[arg(long, env = "PROMETHEUS_LEARNING_QUEUE")]
    queue_root: Option<PathBuf>,
    #[arg(
        long,
        env = "SURREAL_MEMORY_URL",
        default_value = "http://127.0.0.1:23001"
    )]
    memory_url: String,
    #[command(subcommand)]
    command: Option<WorkerCommand>,
}

#[derive(Debug, Subcommand)]
enum WorkerCommand {
    /// Process all currently available jobs and exit.
    RunOnce,
    /// Print the current worker and queue state.
    Status {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearningJob {
    schema_version: u32,
    event_id: String,
    event_type: String,
    harness: String,
    session_id: String,
    project_root: PathBuf,
    transcript_path: Option<PathBuf>,
    captured_at: String,
    payload_digest: String,
    #[serde(default)]
    scope: LearningScope,
    #[serde(default)]
    attempt: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LearningScope {
    #[default]
    Project,
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryOperation {
    schema_version: u32,
    operation_id: String,
    method: String,
    arguments: Value,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    payload_hash: Option<String>,
    #[serde(default = "default_delivery_state")]
    state: String,
    queued_at: String,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    receipt: Option<OperationReceipt>,
}

fn default_delivery_state() -> String {
    "pending".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct OperationReceipt {
    operation_id: String,
    schema_version: u32,
    kind: String,
    payload_hash: String,
    dependencies: Vec<String>,
    state: String,
    blocked_by: Vec<String>,
    result: Option<Value>,
    error: Option<String>,
    executor_generation: u64,
    progress_seq: u64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    started_at: String,
    completed_at: String,
    jobs_completed: usize,
    jobs_rejected: usize,
    memory_delivered: usize,
    memory_awaiting_reconciliation: usize,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueStatus {
    queue_root: String,
    pending: usize,
    processing: usize,
    rejected: usize,
    /// Undrained records created by the legacy retry-count worker.
    retry: usize,
    completed: usize,
    dead_letter: usize,
    memory_pending: usize,
    memory_submitting: usize,
    memory_accepted: usize,
    memory_rejected: usize,
    memory_completed: usize,
    ambiguous_delivery: usize,
    last_run: Option<Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned()))
        .json()
        .init();
    let cli = Cli::parse();
    let queue_root = cli.queue_root.unwrap_or_else(default_queue_root);
    ensure_layout(&queue_root)?;

    match cli.command.unwrap_or(WorkerCommand::RunOnce) {
        WorkerCommand::RunOnce => run_once(&queue_root, &cli.memory_url).await,
        WorkerCommand::Status { json } => print_status(&queue_root, json),
    }
}

fn default_queue_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".prometheus")
        .join("learning-queue")
}

fn ensure_layout(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    harden_directory(root)?;
    for directory in [
        "pending",
        "processing",
        "retry",
        "rejected",
        "completed",
        "dead-letter",
        "memory/pending",
        "memory/submitting",
        "memory/accepted",
        "memory/retry",
        "memory/completed",
        "memory/rejected",
        "memory/dead-letter",
    ] {
        fs::create_dir_all(root.join(directory))?;
        harden_directory(&root.join(directory))?;
    }
    Ok(())
}

async fn run_once(root: &Path, memory_url: &str) -> Result<()> {
    let lock_path = root.join("worker.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    harden_file(&lock_path)?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }

    recover_processing(root)?;
    migrate_legacy_job_retry(root)?;
    recover_memory_submitting(root)?;
    migrate_legacy_memory_retry(root)?;

    let mut summary = RunSummary {
        started_at: Utc::now().to_rfc3339(),
        ..RunSummary::default()
    };
    for path in json_files(&root.join("pending"))? {
        match process_job(root, &path).await {
            Ok(()) => summary.jobs_completed += 1,
            Err(error) => {
                summary.last_error = Some(error.to_string());
                let completed = root
                    .join("completed")
                    .join(path.file_name().context("job has no filename")?);
                if completed.exists() {
                    summary.jobs_completed += 1;
                    continue;
                }
                let processing = root
                    .join("processing")
                    .join(path.file_name().context("job has no filename")?);
                reject_job(root, &processing, &error.to_string())?;
                summary.jobs_rejected += 1;
            }
        }
    }

    let client = reqwest::Client::builder().build()?;
    for directory in ["memory/submitting", "memory/accepted", "memory/pending"] {
        for path in json_files(&root.join(directory))? {
            match reconcile_memory(root, &path, memory_url, &client).await {
                Ok(()) => summary.memory_delivered += 1,
                Err(error) => {
                    summary.last_error = Some(error.to_string());
                    record_memory_error(root, &path, &error.to_string())?;
                    summary.memory_awaiting_reconciliation += 1;
                }
            }
        }
    }
    summary.completed_at = Utc::now().to_rfc3339();
    atomic_json(&root.join("status.json"), &summary)?;
    lock.unlock()?;
    Ok(())
}

fn recover_processing(root: &Path) -> Result<()> {
    for path in json_files(&root.join("processing"))? {
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = root.join("pending").join(name);
        if target.exists() {
            preserve_duplicate(root, &path, "recovered-processing")?;
        } else {
            durable_rename(&path, &target)?;
        }
    }
    Ok(())
}

fn migrate_legacy_job_retry(root: &Path) -> Result<()> {
    for path in json_files(&root.join("retry"))? {
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = root.join("pending").join(name);
        if target.exists() {
            preserve_duplicate(root, &path, "legacy-job-retry")?;
        } else {
            durable_rename(&path, &target)?;
        }
    }
    Ok(())
}

fn recover_memory_submitting(root: &Path) -> Result<()> {
    // A submitting file represents an intentionally ambiguous transport
    // outcome. It stays in place and is reconciled by operation id; it is
    // never blindly moved back to pending.
    for path in json_files(&root.join("memory/submitting"))? {
        let mut operation = read_operation(&path)?;
        operation.state = "submitting".to_owned();
        atomic_json(&path, &operation)?;
    }
    Ok(())
}

fn migrate_legacy_memory_retry(root: &Path) -> Result<()> {
    for path in json_files(&root.join("memory/retry"))? {
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = root.join("memory/pending").join(name);
        if target.exists() {
            preserve_duplicate_in(&root.join("memory/completed"), &path, "legacy-memory-retry")?;
        } else {
            let mut operation = read_operation(&path)?;
            operation.state = "pending".to_owned();
            operation.last_error = None;
            atomic_json(&path, &operation)?;
            durable_rename(&path, &target)?;
        }
    }
    Ok(())
}

async fn process_job(root: &Path, pending_path: &Path) -> Result<()> {
    let name = pending_path
        .file_name()
        .context("job path has no filename")?;
    let processing = root.join("processing").join(name);
    durable_rename(pending_path, &processing)?;
    let raw = fs::read_to_string(&processing)?;
    let job: LearningJob = serde_json::from_str(&raw)?;
    if job.schema_version != 2 {
        anyhow::bail!("unsupported learning job schema {}", job.schema_version);
    }
    let completed = root.join("completed").join(name);
    if completed.exists() {
        preserve_duplicate(root, &processing, "duplicate-completed")?;
        return Ok(());
    }

    let packet = build_session_packet(&job)?;
    let target_kb = match job.scope {
        LearningScope::Project => job.project_root.join(".prometheus/knowledge"),
        LearningScope::Shared => dirs::home_dir()
            .context("HOME unavailable")?
            .join(".prometheus/knowledge/shared"),
    };
    let store = MarkdownStore::open(&target_kb).await?;
    let entry_id = format!(
        "karpathy-session-{}",
        &job.event_id[..16.min(job.event_id.len())]
    );
    let article_id = pk_core::ArticleId::from(entry_id.clone());
    if store.get(&article_id).await.is_err() {
        let mut entry = WikiEntry::new(
            format!(
                "Karpathy session {}",
                &job.event_id[..12.min(job.event_id.len())]
            ),
            packet.clone(),
        );
        entry.id = article_id;
        entry.entry_type = Some("SessionRecord".to_owned());
        entry.tags = vec!["karpathy".to_owned(), "session-learning".to_owned()];
        entry.sources = vec![format!("session:{}", job.session_id)];
        store.upsert(entry).await?;
        store.regenerate_index().await?;
        store
            .append_log(
                "Ingest",
                &format!(
                    "Karpathy session {}",
                    &job.event_id[..12.min(job.event_id.len())]
                ),
                &pk_core::ArticleId::from(entry_id),
            )
            .await?;
    }
    append_learning_log(&job, &packet)?;
    enqueue_memory(root, &job, &packet)?;
    durable_rename(&processing, &completed)?;
    Ok(())
}

fn build_session_packet(job: &LearningJob) -> Result<String> {
    let final_message = job
        .transcript_path
        .as_deref()
        .and_then(extract_final_assistant_message)
        .unwrap_or_else(|| "Session completed; no final assistant text was available.".to_owned());
    let changed_paths = git_changed_paths(&job.project_root);
    let paths = if changed_paths.is_empty() {
        "- No changed paths detected.".to_owned()
    } else {
        changed_paths
            .into_iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(format!(
        "## Delta\n\n{}\n\n## Root Cause\n\nNo explicit root-cause section was captured; preserve this as a session record, not an inferred diagnosis.\n\n## Corrective Actions\n\nReview and promote only reusable findings.\n\n## Session Metadata\n\n- Harness: {}\n- Session: {}\n- Captured: {}\n- Project: {}\n\n## Changed Paths\n\n{}\n",
        truncate_chars(&final_message, 4_000),
        job.harness,
        job.session_id,
        job.captured_at,
        job.project_root.display(),
        paths
    ))
}

fn extract_final_assistant_message(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut result = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let role = value
            .pointer("/message/role")
            .or_else(|| value.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if role != "assistant" && kind != "assistant" {
            continue;
        }
        if let Some(text) = extract_text(&value) {
            result = Some(text);
        }
    }
    result
}

fn extract_text(value: &Value) -> Option<String> {
    for pointer in ["/message/content", "/content", "/message/text", "/text"] {
        let Some(candidate) = value.pointer(pointer) else {
            continue;
        };
        if let Some(text) = candidate.as_str() {
            return Some(text.to_owned());
        }
        if let Some(items) = candidate.as_array() {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn git_changed_paths(project_root: &Path) -> Vec<String> {
    let mut child = match Command::new("git")
        .args(["status", "--short"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    };
    if !status.success() {
        return Vec::new();
    }

    let mut stdout = Vec::new();
    let Some(mut pipe) = child.stdout.take() else {
        return Vec::new();
    };
    if pipe.read_to_end(&mut stdout).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&stdout)
        .lines()
        .take(40)
        .map(|line| line.get(3..).unwrap_or(line).to_owned())
        .collect()
}

fn append_learning_log(job: &LearningJob, packet: &str) -> Result<()> {
    let home = dirs::home_dir().context("HOME unavailable")?;
    let directory = home.join(".prometheus/learning-log");
    fs::create_dir_all(&directory)?;
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let path = directory.join(format!("{date}.jsonl"));
    if path.exists()
        && BufReader::new(File::open(&path)?)
            .lines()
            .map_while(Result::ok)
            .any(|line| line.contains(&job.event_id))
    {
        return Ok(());
    }
    let record = json!({
        "schemaVersion": 2,
        "eventId": job.event_id,
        "sessionId": job.session_id,
        "projectRoot": job.project_root,
        "capturedAt": job.captured_at,
        "processedAt": Utc::now().to_rfc3339(),
        "packetBytes": packet.len()
    });
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, &record)?;
    writeln!(file)?;
    file.sync_data()?;
    Ok(())
}

fn enqueue_memory(root: &Path, job: &LearningJob, packet: &str) -> Result<()> {
    let arguments = json!({
        "content": packet,
        "user_id": match job.scope {
            LearningScope::Project => project_scope(&job.project_root),
            LearningScope::Shared => "global".to_owned(),
        },
        "agent_id": null,
        "session_id": job.session_id,
        "categories": ["karpathy", "session-learning"]
    });
    let operation = MemoryOperation {
        schema_version: 2,
        operation_id: job.event_id.clone(),
        method: "add_memory".to_owned(),
        payload_hash: Some(canonical_payload_hash(&arguments)?),
        arguments,
        dependencies: Vec::new(),
        state: "pending".to_owned(),
        queued_at: Utc::now().to_rfc3339(),
        last_error: None,
        receipt: None,
    };
    let filename = format!("{}.json", operation.operation_id);
    let path = root.join("memory/pending").join(&filename);
    if !path.exists() && !root.join("memory/completed").join(&filename).exists() {
        atomic_json(&path, &operation)?;
    }
    Ok(())
}

async fn reconcile_memory(
    root: &Path,
    path: &Path,
    memory_url: &str,
    client: &reqwest::Client,
) -> Result<()> {
    let mut current_path = path.to_path_buf();
    let mut operation = read_operation(&current_path)?;
    if operation.state == "pending" {
        let target = root.join("memory/submitting").join(
            current_path
                .file_name()
                .context("memory operation has no filename")?,
        );
        operation.state = "submitting".to_owned();
        operation.last_error = None;
        atomic_json(&current_path, &operation)?;
        durable_rename(&current_path, &target)?;
        current_path = target;
    }
    let endpoint = format!(
        "{}/api/v2/operations/{}",
        memory_url.trim_end_matches('/'),
        operation.operation_id
    );
    let response = client.get(&endpoint).send().await?;
    let receipt = if response.status() == reqwest::StatusCode::NOT_FOUND {
        ensure_ledger_ready(memory_url, client).await?;
        submit_operation(memory_url, client, &operation).await?
    } else if response.status().is_success() {
        response.json::<OperationReceipt>().await?
    } else {
        anyhow::bail!(
            "operation lookup returned {}: {}",
            response.status(),
            bounded_error(&response.text().await.unwrap_or_default())
        );
    };
    apply_receipt(root, &current_path, &mut operation, receipt)
}

async fn ensure_ledger_ready(memory_url: &str, client: &reqwest::Client) -> Result<()> {
    let response = client
        .get(format!("{}/ready", memory_url.trim_end_matches('/')))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success()
        || body
            .pointer("/capabilities/ledger")
            .and_then(Value::as_bool)
            != Some(true)
    {
        anyhow::bail!("memory operation ledger is not explicitly ready");
    }
    Ok(())
}

async fn submit_operation(
    memory_url: &str,
    client: &reqwest::Client,
    operation: &MemoryOperation,
) -> Result<OperationReceipt> {
    let payload_hash = operation
        .payload_hash
        .as_deref()
        .context("normalized operation is missing payload_hash")?;
    let response = client
        .post(format!(
            "{}/api/v2/operations",
            memory_url.trim_end_matches('/')
        ))
        .json(&json!({
            "operation_id": operation.operation_id,
            "schema_version": 2,
            "kind": operation.method,
            "dependencies": operation.dependencies,
            "payload_hash": payload_hash,
            "payload": operation.arguments
        }))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response.json().await?);
    }
    let detail = response.text().await.unwrap_or_default();
    anyhow::bail!(
        "operation submission returned {status}: {}",
        bounded_error(&detail)
    )
}

fn apply_receipt(
    root: &Path,
    path: &Path,
    operation: &mut MemoryOperation,
    receipt: OperationReceipt,
) -> Result<()> {
    let expected_hash = operation
        .payload_hash
        .as_deref()
        .context("normalized operation is missing payload_hash")?;
    if receipt.schema_version != 2
        || receipt.operation_id != operation.operation_id
        || receipt.kind != operation.method
        || receipt.payload_hash != expected_hash
        || receipt.dependencies != operation.dependencies
    {
        anyhow::bail!("receipt contract, identity, or payload hash does not match local operation");
    }
    operation.receipt = Some(receipt.clone());
    operation.last_error = receipt.error.clone();
    let destination = match receipt.state.as_str() {
        "committed" => {
            operation.state = "completed".to_owned();
            root.join("memory/completed")
        }
        "rejected" => {
            operation.state = "rejected".to_owned();
            root.join("memory/rejected")
        }
        "accepted" | "validated" | "blocked" | "planned" | "processing" | "indexed" => {
            operation.state = "accepted".to_owned();
            root.join("memory/accepted")
        }
        state => anyhow::bail!("operation receipt has unknown state {state}"),
    };
    atomic_json(path, operation)?;
    let target = destination.join(
        path.file_name()
            .context("memory operation has no filename")?,
    );
    if path != target {
        if target.exists() {
            preserve_duplicate_in(
                &root.join("memory/completed"),
                path,
                "receipt-reconciliation",
            )?;
        } else {
            durable_rename(path, &target)?;
        }
    }
    Ok(())
}

fn read_operation(path: &Path) -> Result<MemoryOperation> {
    let mut operation: MemoryOperation = serde_json::from_slice(&fs::read(path)?)?;
    operation.arguments = normalize_payload(&operation.method, &operation.arguments)?;
    operation.schema_version = 2;
    let computed_hash = canonical_payload_hash(&operation.arguments)?;
    if operation
        .payload_hash
        .as_deref()
        .is_some_and(|stored_hash| stored_hash != computed_hash)
    {
        anyhow::bail!("stored payload hash does not match normalized operation payload");
    }
    operation.payload_hash = Some(computed_hash);
    if operation.state.trim().is_empty() {
        operation.state = "pending".to_owned();
    }
    Ok(operation)
}

fn normalize_payload(method: &str, arguments: &Value) -> Result<Value> {
    match method {
        "add_memory" | "create_task_stream" => Ok(arguments.clone()),
        "add_task_step" if arguments.get("stream_name").is_some() => Ok(arguments.clone()),
        "add_task_step" => {
            let stream = arguments
                .get("stream")
                .and_then(Value::as_str)
                .context("add_task_step arguments require stream")?;
            let description = arguments
                .get("description")
                .and_then(Value::as_str)
                .context("add_task_step arguments require description")?;
            Ok(json!({
                "stream_name": stream,
                "ordinal": 1,
                "name": description,
                "description": description,
                "idempotency_key": description,
                "agent_id": null,
                "user_id": null
            }))
        }
        "complete_step" if arguments.get("idempotency_key").is_some() => Ok(arguments.clone()),
        "complete_step" => {
            let step = arguments
                .get("step")
                .and_then(Value::as_str)
                .context("complete_step arguments require step")?;
            Ok(json!({"idempotency_key":step,"result":"completed via memory bridge"}))
        }
        other => anyhow::bail!("unsupported memory method {other}"),
    }
}

fn canonical_payload_hash(payload: &Value) -> Result<String> {
    let encoded = serde_json::to_vec(payload)?;
    Ok(Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn bounded_error(detail: &str) -> String {
    detail.chars().take(500).collect()
}

fn reject_job(root: &Path, processing: &Path, error: &str) -> Result<()> {
    let name = processing.file_name().context("job has no filename")?;
    let rejected = root.join("rejected").join(name);
    let source = rejected.with_extension("source.json");
    let failure = rejected.with_extension("failure.json");
    let original = fs::read(processing)?;
    let source_hash = Sha256::digest(&original)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    atomic_json(
        &failure,
        &json!({
            "schemaVersion": 2,
            "state": "rejected",
            "sourceHash": source_hash,
            "error": bounded_error(error),
            "rejectedAt": Utc::now().to_rfc3339()
        }),
    )?;
    durable_rename(processing, &source)
}

fn record_memory_error(root: &Path, original_path: &Path, error: &str) -> Result<()> {
    let path = locate_memory_operation(root, original_path)?;
    let mut operation = read_operation(&path)?;
    operation.last_error = Some(error.to_owned());
    atomic_json(&path, &operation)
}

fn locate_memory_operation(root: &Path, original_path: &Path) -> Result<PathBuf> {
    if original_path.exists() {
        return Ok(original_path.to_path_buf());
    }
    let name = original_path
        .file_name()
        .context("memory operation has no filename")?;
    for state in ["submitting", "accepted", "pending", "completed", "rejected"] {
        let candidate = root.join("memory").join(state).join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "memory operation {} is absent from every durable local state",
        name.to_string_lossy()
    )
}

fn preserve_duplicate(root: &Path, path: &Path, label: &str) -> Result<()> {
    preserve_duplicate_in(&root.join("completed"), path, label)
}

fn preserve_duplicate_in(directory: &Path, path: &Path, label: &str) -> Result<()> {
    let name = path.file_name().context("duplicate path has no filename")?;
    let target = directory.join(format!(
        "{}.{}.{}",
        name.to_string_lossy(),
        label,
        Utc::now().timestamp_millis()
    ));
    durable_rename(path, &target)?;
    Ok(())
}

fn json_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .context("output path has no filename")?
            .to_string_lossy(),
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    harden_file(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    sync_directory(parent)?;
    Ok(())
}

fn durable_rename(source: &Path, target: &Path) -> Result<()> {
    let source_parent = source.parent().context("source path has no parent")?;
    let target_parent = target.parent().context("target path has no parent")?;
    fs::rename(source, target)?;
    sync_directory(target_parent)?;
    if source_parent != target_parent {
        sync_directory(source_parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn print_status(root: &Path, json_output: bool) -> Result<()> {
    let status = QueueStatus {
        queue_root: root.display().to_string(),
        pending: json_files(&root.join("pending"))?.len(),
        processing: json_files(&root.join("processing"))?.len(),
        rejected: json_files(&root.join("rejected"))?.len(),
        retry: json_files(&root.join("retry"))?.len(),
        completed: json_files(&root.join("completed"))?.len(),
        dead_letter: json_files(&root.join("dead-letter"))?.len(),
        memory_pending: json_files(&root.join("memory/pending"))?.len()
            + json_files(&root.join("memory/retry"))?.len(),
        memory_submitting: json_files(&root.join("memory/submitting"))?.len(),
        memory_accepted: json_files(&root.join("memory/accepted"))?.len(),
        memory_rejected: json_files(&root.join("memory/rejected"))?.len()
            + json_files(&root.join("memory/dead-letter"))?.len(),
        memory_completed: json_files(&root.join("memory/completed"))?.len(),
        ambiguous_delivery: 0,
        last_run: fs::read(root.join("status.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok()),
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("pending: {}", status.pending);
        println!("processing: {}", status.processing);
        println!("rejected: {}", status.rejected);
        println!("retry: {}", status.retry);
        println!("completed: {}", status.completed);
        println!("dead-letter: {}", status.dead_letter);
        println!("memory pending: {}", status.memory_pending);
        println!("memory submitting: {}", status.memory_submitting);
        println!("memory accepted: {}", status.memory_accepted);
        println!("memory completed: {}", status.memory_completed);
        println!("memory rejected: {}", status.memory_rejected);
        println!("ambiguous delivery: {}", status.ambiguous_delivery);
    }
    Ok(())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn project_scope(project_root: &Path) -> String {
    let manifest = project_root.join(".prometheus/project.json");
    if let Ok(raw) = fs::read(&manifest) {
        if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
            if let Some(id) = value
                .get("projectId")
                .or_else(|| value.get("project_id"))
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            {
                return id.to_owned();
            }
        }
    }
    let stable_path = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let digest = Sha256::digest(stable_path.to_string_lossy().as_bytes());
    format!(
        "project:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[cfg(unix)]
fn harden_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn harden_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path as AxumPath, State},
        http::StatusCode,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn learning_job(project_root: PathBuf) -> LearningJob {
        LearningJob {
            schema_version: 2,
            event_id: "0123456789abcdef0123456789abcdef".to_owned(),
            event_type: "stop".to_owned(),
            harness: "fixture".to_owned(),
            session_id: "fixture-session".to_owned(),
            project_root,
            transcript_path: None,
            captured_at: "2026-08-03T00:00:00Z".to_owned(),
            payload_digest: "fixture".to_owned(),
            scope: LearningScope::Project,
            attempt: 0,
        }
    }

    fn operation(method: &str, arguments: Value) -> MemoryOperation {
        MemoryOperation {
            schema_version: 2,
            operation_id: "operation-1".to_owned(),
            method: method.to_owned(),
            arguments,
            dependencies: Vec::new(),
            payload_hash: None,
            state: "pending".to_owned(),
            queued_at: "2026-08-03T00:00:00Z".to_owned(),
            last_error: None,
            receipt: None,
        }
    }

    fn receipt(operation: &MemoryOperation, state: &str) -> OperationReceipt {
        OperationReceipt {
            operation_id: operation.operation_id.clone(),
            schema_version: 2,
            kind: operation.method.clone(),
            payload_hash: operation.payload_hash.clone().unwrap(),
            dependencies: Vec::new(),
            state: state.to_owned(),
            blocked_by: Vec::new(),
            result: (state == "committed").then(|| json!({"id":"memory:operation-1"})),
            error: None,
            executor_generation: 1,
            progress_seq: 6,
            created_at: "2026-08-03T00:00:00Z".to_owned(),
            updated_at: "2026-08-03T00:00:01Z".to_owned(),
        }
    }

    #[derive(Clone)]
    struct LedgerFixture {
        lookup: Option<OperationReceipt>,
        submission: OperationReceipt,
        ready: bool,
        posts: Arc<Mutex<usize>>,
    }

    async fn ledger_lookup(
        State(state): State<LedgerFixture>,
        AxumPath(_operation_id): AxumPath<String>,
    ) -> Result<Json<OperationReceipt>, StatusCode> {
        state.lookup.map(Json).ok_or(StatusCode::NOT_FOUND)
    }

    async fn ledger_ready(State(state): State<LedgerFixture>) -> Json<Value> {
        Json(json!({"capabilities":{"ledger":state.ready}}))
    }

    async fn ledger_submit(
        State(state): State<LedgerFixture>,
        Json(_body): Json<Value>,
    ) -> Json<OperationReceipt> {
        *state.posts.lock().unwrap() += 1;
        Json(state.submission)
    }

    async fn serve_ledger(state: LedgerFixture) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/ready", get(ledger_ready))
            .route("/api/v2/operations", post(ledger_submit))
            .route("/api/v2/operations/{operation_id}", get(ledger_lookup))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), server)
    }

    #[test]
    fn payload_hash_is_stable_without_character_chunking() {
        let payload = json!({"content":"Delta 🦀 root cause and corrective action".repeat(80)});
        assert_eq!(
            canonical_payload_hash(&payload).unwrap(),
            canonical_payload_hash(&payload).unwrap()
        );
    }

    #[test]
    fn maps_legacy_task_step_arguments_to_v2_contract() {
        let add = normalize_payload(
            "add_task_step",
            &json!({"stream":"legacy:test:phase","description":"change-001"}),
        )
        .unwrap();
        assert_eq!(add["stream_name"], "legacy:test:phase");
        assert_eq!(add["idempotency_key"], "change-001");
        assert_eq!(add["ordinal"], 1);

        let complete = normalize_payload(
            "complete_step",
            &json!({"stream":"legacy:test:phase","step":"change-001"}),
        )
        .unwrap();
        assert_eq!(complete["idempotency_key"], "change-001");
    }

    #[test]
    fn terminal_receipt_moves_operation_exactly_once() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let path = temp.path().join("memory/submitting/operation-1.json");
        atomic_json(&path, &operation).unwrap();
        let receipt = receipt(&operation, "committed");
        apply_receipt(temp.path(), &path, &mut operation, receipt).unwrap();
        assert!(!path.exists());
        let completed = temp.path().join("memory/completed/operation-1.json");
        assert!(completed.exists());
        let stored = read_operation(&completed).unwrap();
        assert_eq!(stored.state, "completed");
        assert_eq!(stored.receipt.unwrap().state, "committed");
    }

    #[test]
    fn local_payload_hash_mismatch_is_rejected_before_transport() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some("0".repeat(64));
        let path = temp.path().join("memory/pending/operation-1.json");
        atomic_json(&path, &operation).unwrap();

        let error = read_operation(&path).unwrap_err().to_string();

        assert!(error.contains("stored payload hash"), "{error}");
    }

    #[test]
    fn receipt_dependencies_must_match_the_local_operation() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let path = temp.path().join("memory/submitting/operation-1.json");
        atomic_json(&path, &operation).unwrap();
        let mut mismatched = receipt(&operation, "accepted");
        mismatched.dependencies = vec!["unexpected".to_owned()];

        let error = apply_receipt(temp.path(), &path, &mut operation, mismatched)
            .unwrap_err()
            .to_string();

        assert!(error.contains("receipt contract"), "{error}");
        assert!(path.exists());
    }

    #[test]
    fn scope_fallback_distinguishes_same_named_checkouts() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("one/project");
        let second = temp.path().join("two/project");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        assert_ne!(project_scope(&first), project_scope(&second));
        assert_eq!(project_scope(&first), project_scope(&first));
    }

    #[test]
    fn transcript_text_cannot_promote_memory_to_global_scope() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let job = learning_job(project.clone());

        enqueue_memory(
            temp.path(),
            &job,
            "ordinary assistant prose containing the untrusted marker [GLOBAL]",
        )
        .unwrap();

        let operation = read_operation(
            &temp
                .path()
                .join("memory/pending/0123456789abcdef0123456789abcdef.json"),
        )
        .unwrap();
        assert_eq!(operation.arguments["user_id"], project_scope(&project));
    }

    #[test]
    fn legacy_job_retry_is_migrated_without_changing_identity() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let job = learning_job(temp.path().join("project"));
        let retry = temp.path().join("retry/job.json");
        atomic_json(&retry, &job).unwrap();

        migrate_legacy_job_retry(temp.path()).unwrap();

        assert!(!retry.exists());
        let pending = temp.path().join("pending/job.json");
        assert!(pending.exists());
        let migrated: LearningJob = serde_json::from_slice(&fs::read(pending).unwrap()).unwrap();
        assert_eq!(migrated.event_id, job.event_id);
        assert_eq!(migrated.attempt, job.attempt);
    }

    #[test]
    fn legacy_memory_retry_is_migrated_under_the_same_operation_id() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"existing record"}));
        operation.operation_id = "existing-operation-id".to_owned();
        operation.state = "retry".to_owned();
        operation.last_error = Some("legacy transport failure".to_owned());
        let retry = temp.path().join("memory/retry/existing-operation-id.json");
        atomic_json(&retry, &operation).unwrap();

        migrate_legacy_memory_retry(temp.path()).unwrap();

        assert!(!retry.exists());
        let pending = temp
            .path()
            .join("memory/pending/existing-operation-id.json");
        let migrated = read_operation(&pending).unwrap();
        assert_eq!(migrated.operation_id, "existing-operation-id");
        assert_eq!(migrated.state, "pending");
        assert!(migrated.last_error.is_none());
    }

    #[test]
    fn malformed_job_is_rejected_with_its_original_bytes() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let processing = temp.path().join("processing/broken.json");
        let original = br#"{"schemaVersion":2,"eventId":"unterminated"#;
        fs::write(&processing, original).unwrap();

        reject_job(temp.path(), &processing, "malformed fixture").unwrap();

        assert!(!processing.exists());
        assert_eq!(
            fs::read(temp.path().join("rejected/broken.source.json")).unwrap(),
            original
        );
        let failure: Value = serde_json::from_slice(
            &fs::read(temp.path().join("rejected/broken.failure.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(failure["error"], "malformed fixture");
    }

    #[tokio::test]
    async fn existing_ledger_receipt_is_never_resubmitted() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let path = temp.path().join("memory/submitting/operation-1.json");
        operation.state = "submitting".to_owned();
        atomic_json(&path, &operation).unwrap();
        let posts = Arc::new(Mutex::new(0));
        let fixture = LedgerFixture {
            lookup: Some(receipt(&operation, "committed")),
            submission: receipt(&operation, "accepted"),
            ready: true,
            posts: Arc::clone(&posts),
        };
        let (url, server) = serve_ledger(fixture).await;

        reconcile_memory(temp.path(), &path, &url, &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(*posts.lock().unwrap(), 0);
        assert!(temp
            .path()
            .join("memory/completed/operation-1.json")
            .exists());
        server.abort();
    }

    #[tokio::test]
    async fn absent_operation_is_not_submitted_until_ledger_is_ready() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let path = temp.path().join("memory/submitting/operation-1.json");
        operation.state = "submitting".to_owned();
        atomic_json(&path, &operation).unwrap();
        let posts = Arc::new(Mutex::new(0));
        let fixture = LedgerFixture {
            lookup: None,
            submission: receipt(&operation, "accepted"),
            ready: false,
            posts: Arc::clone(&posts),
        };
        let (url, server) = serve_ledger(fixture).await;

        let result = reconcile_memory(temp.path(), &path, &url, &reqwest::Client::new()).await;

        assert!(result.is_err());
        assert_eq!(*posts.lock().unwrap(), 0);
        assert!(path.exists());
        server.abort();
    }

    #[tokio::test]
    async fn authoritative_absence_submits_once_and_persists_acceptance() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let path = temp.path().join("memory/submitting/operation-1.json");
        operation.state = "submitting".to_owned();
        atomic_json(&path, &operation).unwrap();
        let posts = Arc::new(Mutex::new(0));
        let fixture = LedgerFixture {
            lookup: None,
            submission: receipt(&operation, "accepted"),
            ready: true,
            posts: Arc::clone(&posts),
        };
        let (url, server) = serve_ledger(fixture).await;

        reconcile_memory(temp.path(), &path, &url, &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(*posts.lock().unwrap(), 1);
        let accepted = temp.path().join("memory/accepted/operation-1.json");
        assert!(accepted.exists());
        assert_eq!(read_operation(&accepted).unwrap().state, "accepted");
        server.abort();
    }
}
