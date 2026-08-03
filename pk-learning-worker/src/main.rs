use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use fs2::FileExt;
use pk_core::WikiEntry;
use pk_store::MarkdownStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

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
    attempt: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryOperation {
    schema_version: u32,
    operation_id: String,
    method: String,
    arguments: Value,
    attempt: u32,
    queued_at: String,
    last_error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    started_at: String,
    completed_at: String,
    jobs_completed: usize,
    jobs_retried: usize,
    jobs_dead_lettered: usize,
    memory_delivered: usize,
    memory_retried: usize,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueStatus {
    queue_root: String,
    pending: usize,
    processing: usize,
    retry: usize,
    completed: usize,
    dead_letter: usize,
    memory_pending: usize,
    memory_dead_letter: usize,
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
        "completed",
        "dead-letter",
        "memory/pending",
        "memory/retry",
        "memory/completed",
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
        .read(true)
        .write(true)
        .open(&lock_path)?;
    harden_file(&lock_path)?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }

    recover_processing(root)?;
    promote_retry(root, "retry", "pending")?;
    promote_retry(root, "memory/retry", "memory/pending")?;

    let mut summary = RunSummary {
        started_at: Utc::now().to_rfc3339(),
        ..RunSummary::default()
    };
    for path in json_files(&root.join("pending"))? {
        match process_job(root, &path).await {
            Ok(()) => summary.jobs_completed += 1,
            Err(error) => {
                summary.last_error = Some(error.to_string());
                if retry_job(root, &path, &error.to_string())? {
                    summary.jobs_retried += 1;
                } else {
                    summary.jobs_dead_lettered += 1;
                }
            }
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(20))
        .build()?;
    for path in json_files(&root.join("memory/pending"))? {
        match deliver_memory(root, &path, memory_url, &client).await {
            Ok(()) => summary.memory_delivered += 1,
            Err(error) => {
                summary.last_error = Some(error.to_string());
                retry_memory(root, &path, &error.to_string())?;
                summary.memory_retried += 1;
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
            fs::rename(path, target)?;
        }
    }
    Ok(())
}

fn promote_retry(root: &Path, from: &str, to: &str) -> Result<()> {
    for path in json_files(&root.join(from))? {
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = root.join(to).join(name);
        if target.exists() {
            preserve_duplicate(root, &path, "duplicate-retry")?;
        } else {
            fs::rename(path, target)?;
        }
    }
    Ok(())
}

async fn process_job(root: &Path, pending_path: &Path) -> Result<()> {
    let name = pending_path
        .file_name()
        .context("job path has no filename")?;
    let processing = root.join("processing").join(name);
    fs::rename(pending_path, &processing)?;
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
    let target_kb = if packet.contains("[GLOBAL]") {
        dirs::home_dir()
            .context("HOME unavailable")?
            .join(".prometheus/knowledge/shared")
    } else {
        job.project_root.join(".prometheus/knowledge")
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
    fs::rename(processing, completed)?;
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
    let output = Command::new("git")
        .args(["status", "--short"])
        .current_dir(project_root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
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
    let operation = MemoryOperation {
        schema_version: 1,
        operation_id: job.event_id.clone(),
        method: "add_memory".to_owned(),
        arguments: json!({
            "content": packet,
            "user_id": if packet.contains("[GLOBAL]") {
                "global".to_owned()
            } else {
                project_scope(&job.project_root)
            },
            "metadata": {"source":"prometheus-learning-worker","event_id":job.event_id}
        }),
        attempt: 0,
        queued_at: Utc::now().to_rfc3339(),
        last_error: None,
    };
    let path = root
        .join("memory/pending")
        .join(format!("{}.json", operation.operation_id));
    if !path.exists()
        && !root
            .join("memory/completed")
            .join(path.file_name().unwrap())
            .exists()
    {
        atomic_json(&path, &operation)?;
    }
    Ok(())
}

async fn deliver_memory(
    root: &Path,
    path: &Path,
    memory_url: &str,
    client: &reqwest::Client,
) -> Result<()> {
    let operation: MemoryOperation = serde_json::from_slice(&fs::read(path)?)?;
    if operation.method != "add_memory" {
        anyhow::bail!("unsupported memory method {}", operation.method);
    }
    let response = client
        .post(format!(
            "{}/api/v1/memory",
            memory_url.trim_end_matches('/')
        ))
        .json(&operation.arguments)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("memory endpoint returned {}", response.status());
    }
    let target = root.join("memory/completed").join(
        path.file_name()
            .context("memory operation has no filename")?,
    );
    fs::rename(path, target)?;
    Ok(())
}

fn retry_job(root: &Path, path: &Path, error: &str) -> Result<bool> {
    let processing = if path.starts_with(root.join("processing")) {
        path.to_path_buf()
    } else {
        root.join("processing")
            .join(path.file_name().context("job has no filename")?)
    };
    let mut job: LearningJob = serde_json::from_slice(&fs::read(&processing)?)?;
    job.attempt += 1;
    if job.attempt >= 5 {
        let target = root
            .join("dead-letter")
            .join(processing.file_name().context("job has no filename")?);
        atomic_json(
            &target,
            &json!({"job":job,"error":error,"failedAt":Utc::now()}),
        )?;
        fs::rename(processing, target.with_extension("source.json"))?;
        return Ok(false);
    }
    atomic_json(&processing, &job)?;
    let target = root
        .join("retry")
        .join(processing.file_name().context("job has no filename")?);
    fs::rename(processing, target)?;
    Ok(true)
}

fn retry_memory(root: &Path, path: &Path, error: &str) -> Result<()> {
    let mut operation: MemoryOperation = serde_json::from_slice(&fs::read(path)?)?;
    operation.attempt += 1;
    operation.last_error = Some(error.to_owned());
    if operation.attempt >= 8 {
        let target = root
            .join("memory/dead-letter")
            .join(path.file_name().context("operation has no filename")?);
        atomic_json(path, &operation)?;
        fs::rename(path, target)?;
    } else {
        atomic_json(path, &operation)?;
        let target = root
            .join("memory/retry")
            .join(path.file_name().context("operation has no filename")?);
        fs::rename(path, target)?;
    }
    Ok(())
}

fn preserve_duplicate(root: &Path, path: &Path, label: &str) -> Result<()> {
    let name = path.file_name().context("duplicate path has no filename")?;
    let target = root.join("completed").join(format!(
        "{}.{}.{}",
        name.to_string_lossy(),
        label,
        Utc::now().timestamp_millis()
    ));
    fs::rename(path, target)?;
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
        ".{}.{}.tmp",
        path.file_name()
            .context("output path has no filename")?
            .to_string_lossy(),
        std::process::id()
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
    Ok(())
}

fn print_status(root: &Path, json_output: bool) -> Result<()> {
    let status = QueueStatus {
        queue_root: root.display().to_string(),
        pending: json_files(&root.join("pending"))?.len(),
        processing: json_files(&root.join("processing"))?.len(),
        retry: json_files(&root.join("retry"))?.len(),
        completed: json_files(&root.join("completed"))?.len(),
        dead_letter: json_files(&root.join("dead-letter"))?.len(),
        memory_pending: json_files(&root.join("memory/pending"))?.len()
            + json_files(&root.join("memory/retry"))?.len(),
        memory_dead_letter: json_files(&root.join("memory/dead-letter"))?.len(),
        last_run: fs::read(root.join("status.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok()),
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("pending: {}", status.pending);
        println!("processing: {}", status.processing);
        println!("retry: {}", status.retry);
        println!("completed: {}", status.completed);
        println!("dead-letter: {}", status.dead_letter);
        println!("memory pending: {}", status.memory_pending);
        println!("memory dead-letter: {}", status.memory_dead_letter);
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
    project_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned()
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
