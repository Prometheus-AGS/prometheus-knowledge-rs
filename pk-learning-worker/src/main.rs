use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use pk_core::WikiEntry;
use pk_store::MarkdownStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

mod promotion;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MEMORY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// First wait before re-polling an accepted operation whose receipt did not change.
const POLL_BACKOFF_BASE_SECONDS: u64 = 60;
/// Longest wait between polls, so a stranded operation still recovers within an
/// hour of the server finishing it.
const POLL_BACKOFF_CAP_SECONDS: u64 = 3_600;

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
    /// How long an accepted memory operation may go without receipt progress
    /// before it counts as stale (`<n>s|m|h|d`).
    #[arg(
        long,
        env = "PROMETHEUS_LEARNING_STALE_AFTER",
        default_value = "6h",
        value_parser = parse_duration
    )]
    stale_after: Duration,
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
    /// Move stale accepted memory operations to `memory/stalled`, out of the
    /// redelivery loop, and record them in a manifest.
    Quarantine {
        /// Staleness threshold for this command; defaults to `--stale-after`.
        #[arg(long, value_parser = parse_duration)]
        older_than: Option<Duration>,
        /// List the operations that would move and change nothing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Return stalled memory operations to the redelivery loop.
    Release {
        /// Release every stalled operation.
        #[arg(long, default_value_t = false, conflicts_with = "operation_ids")]
        all: bool,
        /// Operation ids to release.
        #[arg(required_unless_present = "all")]
        operation_ids: Vec<String>,
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
    /// Resolved by the hook's single project-id resolver; falls back to
    /// `project_scope(project_root)` when absent (jobs queued by older hooks).
    #[serde(default)]
    project_id: Option<String>,
    /// Agent-team id of the authoring agent, when it resolved to a team role.
    #[serde(default)]
    team_id: Option<String>,
    /// Role id within `team_id`.
    #[serde(default)]
    role_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LearningScope {
    #[default]
    Project,
    Shared,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// When the server first accepted the operation, from its receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_accepted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_receipt_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_receipt_progress_seq: Option<u64>,
    /// When the receipt state or progress sequence last changed. Staleness is
    /// measured from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_receipt_change_at: Option<String>,
    /// Consecutive polls that returned an unchanged receipt.
    #[serde(default, skip_serializing_if = "is_zero")]
    unchanged_polls: u32,
    /// Earliest time an unchanged accepted operation is polled again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_poll_at: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What a receipt did to a local operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptOutcome {
    /// The operation moved to a new local state (accepted, completed, rejected).
    Transitioned,
    /// Still accepted, but the server reported progress.
    Progressed,
    /// Still accepted, and the receipt is the same as last time.
    Unchanged,
}

fn default_delivery_state() -> String {
    "pending".to_owned()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Operations that moved to a new local state during this run.
    memory_delivered: usize,
    /// Operations polled this run whose receipt is still non-terminal.
    memory_in_flight: usize,
    /// Accepted operations skipped this run because they are backing off.
    memory_deferred: usize,
    memory_awaiting_reconciliation: usize,
    /// Accepted operations with no receipt progress for longer than the threshold.
    memory_stale: usize,
    /// Operations quarantined in `memory/stalled`.
    memory_stalled: usize,
    oldest_accepted_age_seconds: Option<i64>,
    stale_after_seconds: u64,
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
    /// Quarantined operations. Kept apart from the unhealthy counts.
    memory_stalled: usize,
    memory_stale: usize,
    oldest_accepted_age_seconds: Option<i64>,
    stale_after_seconds: u64,
    ambiguous_delivery: usize,
    last_run: Option<Value>,
}

/// One operation moved (or, in a dry run, selected) by `quarantine` or `release`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEntry {
    operation_id: String,
    from: String,
    to: String,
    last_receipt_state: Option<String>,
    stale_since: Option<String>,
    stale_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    action: &'static str,
    created_at: String,
    dry_run: bool,
    older_than_seconds: Option<u64>,
    operations: Vec<ManifestEntry>,
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
        WorkerCommand::RunOnce => run_once(&queue_root, &cli.memory_url, cli.stale_after).await,
        WorkerCommand::Status { json } => print_status(&queue_root, cli.stale_after, json),
        WorkerCommand::Quarantine {
            older_than,
            dry_run,
            json,
        } => {
            let manifest = quarantine(
                &queue_root,
                older_than.unwrap_or(cli.stale_after),
                dry_run,
                Utc::now(),
            )?;
            print_manifest(&manifest, json)
        }
        WorkerCommand::Release {
            all,
            operation_ids,
            json,
        } => {
            let selection = (!all).then_some(operation_ids.as_slice());
            let manifest = release(&queue_root, selection, Utc::now())?;
            print_manifest(&manifest, json)
        }
    }
}

fn default_queue_root() -> PathBuf {
    pk_core::paths::home_dir()
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
        "memory/stalled",
        "memory/manifests",
    ] {
        fs::create_dir_all(root.join(directory))?;
        harden_directory(&root.join(directory))?;
    }
    Ok(())
}

/// Take the queue lock, or return `None` when another worker holds it.
fn try_worker_lock(root: &Path) -> Result<Option<File>> {
    let lock_path = root.join("worker.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    harden_file(&lock_path)?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(None);
    }
    Ok(Some(lock))
}

async fn run_once(root: &Path, memory_url: &str, stale_after: Duration) -> Result<()> {
    let Some(lock) = try_worker_lock(root)? else {
        return Ok(());
    };

    recover_processing(root)?;
    migrate_legacy_job_retry(root)?;
    recover_memory_submitting(root)?;
    migrate_legacy_memory_retry(root)?;

    let mut summary = RunSummary {
        started_at: Utc::now().to_rfc3339(),
        ..RunSummary::default()
    };
    let mut observations = Vec::new();
    for path in json_files(&root.join("pending"))? {
        match process_job(root, &path).await {
            Ok(observed) => {
                summary.jobs_completed += 1;
                observations.extend(observed);
            }
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
    // Promotion only proposes; a failure here must not hold up memory delivery.
    if let Err(error) = promotion::run(&observations) {
        tracing::warn!(error = %error, "promotion detector failed");
        summary.last_error = Some(format!("promotion detector: {error}"));
    }

    let client = memory_client(MEMORY_REQUEST_TIMEOUT)?;
    // A record reconciled from `memory/submitting` lands in `memory/accepted`
    // before that directory is listed; polling it again would count it twice
    // and start its backoff after a single real observation.
    let mut reconciled = HashSet::new();
    'memory_reconciliation: for directory in
        ["memory/submitting", "memory/accepted", "memory/pending"]
    {
        for path in json_files(&root.join(directory))? {
            let Some(name) = path.file_name().map(ToOwned::to_owned) else {
                continue;
            };
            if !reconciled.insert(name) {
                continue;
            }
            let now = Utc::now();
            if directory == "memory/accepted" && poll_deferred(&path, now) {
                summary.memory_deferred += 1;
                continue;
            }
            match reconcile_memory(root, &path, memory_url, &client, now).await {
                Ok(ReceiptOutcome::Transitioned) => summary.memory_delivered += 1,
                Ok(ReceiptOutcome::Progressed | ReceiptOutcome::Unchanged) => {
                    summary.memory_in_flight += 1
                }
                Err(error) => {
                    summary.last_error = Some(error.to_string());
                    record_memory_error(root, &path, &error.to_string())?;
                    summary.memory_awaiting_reconciliation += 1;
                    if memory_transport_unavailable(&error) {
                        break 'memory_reconciliation;
                    }
                }
            }
        }
    }
    let now = Utc::now();
    let health = accepted_health(root, now, stale_after)?;
    summary.memory_stale = health.stale;
    summary.oldest_accepted_age_seconds = health.oldest_age_seconds;
    summary.memory_stalled = json_files(&root.join("memory/stalled"))?.len();
    summary.stale_after_seconds = stale_after.as_secs();
    summary.completed_at = now.to_rfc3339();
    atomic_json(&root.join("status.json"), &summary)?;
    lock.unlock()?;
    Ok(())
}

fn memory_client(request_timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(request_timeout)
        .build()?)
}

fn memory_transport_unavailable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|error| error.is_timeout() || error.is_connect())
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
        let stored: MemoryOperation = serde_json::from_slice(&fs::read(&path)?)?;
        let mut operation = normalize_operation(stored.clone())?;
        operation.state = "submitting".to_owned();
        if operation != stored {
            atomic_json(&path, &operation)?;
        }
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

async fn process_job(root: &Path, pending_path: &Path) -> Result<Vec<promotion::Observation>> {
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
        return Ok(Vec::new());
    }

    let packet = build_session_packet(&job)?;
    let target_kb = match job.scope {
        LearningScope::Project => job.project_root.join(".prometheus/knowledge"),
        LearningScope::Shared => pk_core::paths::home_dir()
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
        entry.sources = vec![format!("session:{}", job.session_id).into()];
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
    // Publish the store's current entries as the committed prompt snapshot that
    // `pk context` reads. Without this, session records were written to disk
    // but never became recallable.
    let entries = store.snapshot().await?;
    let project_id = lesson_project_id(&job);
    let mut observations = job
        .transcript_path
        .as_deref()
        .and_then(extract_final_assistant_message)
        .map(|message| {
            promotion::observations_from_message(
                &project_id,
                &job.project_root,
                &job.session_id,
                &message,
            )
        })
        .unwrap_or_default();
    if matches!(job.scope, LearningScope::Project) {
        observations.extend(promotion::observations_from_entries(
            &project_id,
            &job.project_root,
            &entries,
        ));
    }
    pk_store::commit_prompt_snapshot(
        &target_kb,
        match job.scope {
            LearningScope::Project => "project",
            LearningScope::Shared => "shared",
        },
        entries,
    )?;
    append_learning_log(&job, &packet)?;
    enqueue_memory(root, &job, &packet)?;
    durable_rename(&processing, &completed)?;
    Ok(observations)
}

/// The project a lesson belongs to, for cross-project recurrence: the job's
/// resolved project id, else the same fallback the memory scope keys use.
fn lesson_project_id(job: &LearningJob) -> String {
    job.project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| project_scope(&job.project_root))
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
        .map(|line| line.get(3..).unwrap_or(line).to_owned())
        // ~keep Drop the collector's own output. Session records are written
        // into `.prometheus/knowledge` (see `target_kb`), so an unfiltered
        // `git status` reports them as the *next* turn's changed work. That fed
        // back on itself: consecutive records were identical except for a
        // growing list of previously-generated karpathy files. Filtering here
        // rather than via .gitignore keeps the wiki reviewable in git for
        // projects that commit it, while still excluding it from "what changed
        // this turn".
        // Untracked output collapses to a bare `.prometheus/` entry, while a
        // project that commits its wiki reports full paths — exclude both forms.
        .filter(|path| {
            let path = path.trim_end_matches('/');
            path != ".prometheus"
                && path != ".prometheus/knowledge"
                && !path.starts_with(".prometheus/knowledge/")
        })
        .take(40)
        .collect()
}

fn append_learning_log(job: &LearningJob, packet: &str) -> Result<()> {
    let home = pk_core::paths::home_dir().context("HOME unavailable")?;
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

/// The surreal-memory scope keys for a job (design: one `agent_id` per
/// visibility level, never null). A role-attributed project lesson is private
/// to `<team>/<role>`; an unattributed one is project-visible; shared-scope
/// learning is global.
fn memory_identity(job: &LearningJob) -> (String, String) {
    let present = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match job.scope {
        LearningScope::Shared => ("@global".to_owned(), "@global".to_owned()),
        LearningScope::Project => {
            let user_id =
                present(&job.project_id).unwrap_or_else(|| project_scope(&job.project_root));
            let agent_id = match (present(&job.team_id), present(&job.role_id)) {
                (Some(team), Some(role)) => format!("{team}/{role}"),
                _ => "@project".to_owned(),
            };
            (user_id, agent_id)
        }
    }
}

fn enqueue_memory(root: &Path, job: &LearningJob, packet: &str) -> Result<()> {
    let (user_id, agent_id) = memory_identity(job);
    let arguments = json!({
        "content": packet,
        "user_id": user_id,
        "agent_id": agent_id,
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
        first_accepted_at: None,
        last_receipt_state: None,
        last_receipt_progress_seq: None,
        last_receipt_change_at: None,
        unchanged_polls: 0,
        next_poll_at: None,
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
    now: DateTime<Utc>,
) -> Result<ReceiptOutcome> {
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
    apply_receipt(root, &current_path, &mut operation, receipt, now)
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
    now: DateTime<Utc>,
) -> Result<ReceiptOutcome> {
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
    let previous_state = operation.state.clone();
    let receipt_changed = observe_receipt(operation, &receipt, now);
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
    Ok(if operation.state != previous_state {
        ReceiptOutcome::Transitioned
    } else if receipt_changed {
        ReceiptOutcome::Progressed
    } else {
        ReceiptOutcome::Unchanged
    })
}

/// Record a receipt in the operation's bookkeeping and schedule the next poll.
/// Returns whether the receipt state or progress sequence changed.
///
/// A record with no bookkeeping (written by an older worker) takes its clocks
/// from the server's receipt timestamps rather than from `now`. Otherwise an
/// operation stranded for days would get a fresh staleness window on upgrade.
fn observe_receipt(
    operation: &mut MemoryOperation,
    receipt: &OperationReceipt,
    now: DateTime<Utc>,
) -> bool {
    let server_time = |raw: &str| {
        parse_time(raw)
            .map_or(now, |time| time.min(now))
            .to_rfc3339()
    };
    if operation.first_accepted_at.is_none() {
        operation.first_accepted_at = Some(server_time(&receipt.created_at));
    }
    let first_observation = operation.last_receipt_state.is_none();
    let changed = operation.last_receipt_state.as_deref() != Some(receipt.state.as_str())
        || operation.last_receipt_progress_seq != Some(receipt.progress_seq);
    if changed {
        operation.last_receipt_change_at = Some(if first_observation {
            server_time(&receipt.updated_at)
        } else {
            now.to_rfc3339()
        });
        operation.last_receipt_state = Some(receipt.state.clone());
        operation.last_receipt_progress_seq = Some(receipt.progress_seq);
        operation.unchanged_polls = 0;
        operation.next_poll_at = None;
    } else {
        operation.unchanged_polls = operation.unchanged_polls.saturating_add(1);
        operation.next_poll_at = Some((now + poll_backoff(operation.unchanged_polls)).to_rfc3339());
    }
    changed
}

/// Exponential backoff for unchanged receipts: 1m, 2m, 4m, ... capped at 1h.
fn poll_backoff(unchanged_polls: u32) -> TimeDelta {
    let doublings = unchanged_polls.saturating_sub(1).min(16);
    let seconds = POLL_BACKOFF_BASE_SECONDS
        .saturating_mul(1_u64 << doublings)
        .min(POLL_BACKOFF_CAP_SECONDS);
    TimeDelta::seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// An accepted operation is skipped while its backoff window is open.
fn poll_deferred(path: &Path, now: DateTime<Utc>) -> bool {
    read_operation(path)
        .ok()
        .and_then(|operation| operation.next_poll_at)
        .and_then(|raw| parse_time(&raw))
        .is_some_and(|next_poll| next_poll > now)
}

fn parse_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn to_time_delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}

/// Parse `<n>s`, `<n>m`, `<n>h`, or `<n>d`.
fn parse_duration(raw: &str) -> std::result::Result<Duration, String> {
    let raw = raw.trim();
    let split = raw
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(raw.len());
    let (digits, unit) = raw.split_at(split);
    let expected = || format!("invalid duration {raw:?}: expected <number><s|m|h|d>, e.g. 6h");
    let value: u64 = digits.parse().map_err(|_| expected())?;
    let unit_seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(expected()),
    };
    if value == 0 {
        // Zero would make every accepted operation stale, even one polled a
        // moment ago.
        return Err(format!("duration {raw:?} must be greater than zero"));
    }
    value
        .checked_mul(unit_seconds)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("duration {raw:?} is too large"))
}

/// When an operation's staleness clock started: the last receipt change, else
/// first acceptance, else when it was queued.
fn stale_since(operation: &MemoryOperation) -> Option<DateTime<Utc>> {
    operation
        .last_receipt_change_at
        .as_deref()
        .or(operation.first_accepted_at.as_deref())
        .and_then(parse_time)
        .or_else(|| parse_time(&operation.queued_at))
}

fn is_stale(operation: &MemoryOperation, now: DateTime<Utc>, threshold: Duration) -> bool {
    stale_since(operation)
        .is_some_and(|since| now.signed_duration_since(since) > to_time_delta(threshold))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AcceptedHealth {
    stale: usize,
    oldest_age_seconds: Option<i64>,
}

/// Count stale accepted operations and the age of the oldest one. Unreadable
/// records are skipped here; reconciliation reports them.
fn accepted_health(root: &Path, now: DateTime<Utc>, threshold: Duration) -> Result<AcceptedHealth> {
    let operations = json_files(&root.join("memory/accepted"))?
        .iter()
        .filter_map(|path| read_operation(path).ok())
        .collect::<Vec<_>>();
    let stale = operations
        .iter()
        .filter(|operation| is_stale(operation, now, threshold))
        .count();
    let oldest_age_seconds = operations
        .iter()
        .filter_map(|operation| {
            operation
                .first_accepted_at
                .as_deref()
                .and_then(parse_time)
                .or_else(|| parse_time(&operation.queued_at))
        })
        .map(|since| now.signed_duration_since(since).num_seconds())
        .max();
    Ok(AcceptedHealth {
        stale,
        oldest_age_seconds,
    })
}

fn quarantine(
    root: &Path,
    older_than: Duration,
    dry_run: bool,
    now: DateTime<Utc>,
) -> Result<Manifest> {
    // A dry run only reads, so it neither takes nor creates the lock.
    let lock = if dry_run {
        None
    } else {
        Some(
            try_worker_lock(root)?
                .context("the learning worker is running; retry when it exits")?,
        )
    };
    let mut selected = Vec::new();
    for path in json_files(&root.join("memory/accepted"))? {
        // Skipped, as in `status`: reconciliation reports unreadable records,
        // and one of them must not block quarantining the rest.
        let operation = match read_operation(&path) {
            Ok(operation) => operation,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skipping unreadable operation");
                continue;
            }
        };
        if is_stale(&operation, now, older_than) {
            selected.push((path, operation));
        }
    }
    let target_for = |path: &Path| -> Result<PathBuf> {
        Ok(root.join("memory/stalled").join(
            path.file_name()
                .context("memory operation has no filename")?,
        ))
    };
    for (path, _) in &selected {
        let target = target_for(path)?;
        if target.exists() {
            anyhow::bail!("{} already exists; nothing was moved", target.display());
        }
    }
    let entries = selected
        .iter()
        .map(|(_, operation)| {
            let since = stale_since(operation);
            ManifestEntry {
                operation_id: operation.operation_id.clone(),
                from: "memory/accepted".to_owned(),
                to: "memory/stalled".to_owned(),
                last_receipt_state: operation.last_receipt_state.clone(),
                stale_since: since.map(|time| time.to_rfc3339()),
                stale_seconds: since.map(|time| now.signed_duration_since(time).num_seconds()),
            }
        })
        .collect();
    let manifest = Manifest {
        schema_version: 1,
        action: "quarantine",
        created_at: now.to_rfc3339(),
        dry_run,
        older_than_seconds: Some(older_than.as_secs()),
        operations: entries,
    };
    let Some(lock) = lock else {
        return Ok(manifest);
    };
    // The manifest is written first, as a record of intent: if a move fails
    // partway, every operation already in `memory/stalled` is still listed.
    write_manifest(root, &manifest)?;
    for (path, mut operation) in selected {
        // Rename before rewriting the state, so a crash in between never
        // leaves a "stalled" record in `memory/accepted`.
        let target = target_for(&path)?;
        durable_rename(&path, &target)?;
        operation.state = "stalled".to_owned();
        atomic_json(&target, &operation)?;
    }
    lock.unlock()?;
    Ok(manifest)
}

/// Move stalled operations back for redelivery: to `memory/accepted` when the
/// server already issued a receipt, otherwise to `memory/pending`. `None`
/// releases everything. Unknown ids fail the command before anything moves.
fn release(root: &Path, operation_ids: Option<&[String]>, now: DateTime<Utc>) -> Result<Manifest> {
    let lock =
        try_worker_lock(root)?.context("the learning worker is running; retry when it exits")?;
    let mut stalled = Vec::new();
    for path in json_files(&root.join("memory/stalled"))? {
        let operation =
            read_operation(&path).with_context(|| format!("cannot read {}", path.display()))?;
        stalled.push((path, operation));
    }
    if let Some(ids) = operation_ids {
        let unknown = ids
            .iter()
            .filter(|id| {
                !stalled
                    .iter()
                    .any(|(_, operation)| &operation.operation_id == *id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            anyhow::bail!("not in memory/stalled: {}", unknown.join(", "));
        }
        stalled.retain(|(_, operation)| ids.contains(&operation.operation_id));
    }
    let mut moves = Vec::with_capacity(stalled.len());
    for (path, operation) in stalled {
        let state = if operation.receipt.is_some() {
            "accepted"
        } else {
            "pending"
        };
        let target = root.join("memory").join(state).join(
            path.file_name()
                .context("memory operation has no filename")?,
        );
        if target.exists() {
            anyhow::bail!("{} already exists; nothing was moved", target.display());
        }
        moves.push((path, target, state, operation));
    }
    let entries = moves
        .iter()
        .map(|(_, _, state, operation)| ManifestEntry {
            operation_id: operation.operation_id.clone(),
            from: "memory/stalled".to_owned(),
            to: format!("memory/{state}"),
            last_receipt_state: operation.last_receipt_state.clone(),
            stale_since: None,
            stale_seconds: None,
        })
        .collect();
    let manifest = Manifest {
        schema_version: 1,
        action: "release",
        created_at: now.to_rfc3339(),
        dry_run: false,
        older_than_seconds: None,
        operations: entries,
    };
    // Written first, as for quarantine, so a partial release is still on record.
    write_manifest(root, &manifest)?;
    for (path, target, state, mut operation) in moves {
        operation.state = state.to_owned();
        operation.unchanged_polls = 0;
        operation.next_poll_at = None;
        if let Some(receipt) = &operation.receipt {
            // Restart the staleness clock so a released record gets a full
            // window. The receipt pair is recorded too: otherwise a record
            // quarantined before its first poll would treat that poll as a
            // first observation and reset the clock to the server's old
            // `updated_at`, making it stale again at once.
            operation.last_receipt_state = Some(receipt.state.clone());
            operation.last_receipt_progress_seq = Some(receipt.progress_seq);
            operation.last_receipt_change_at = Some(now.to_rfc3339());
        }
        // Rewrite before renaming: a crash in between leaves the record in
        // `memory/stalled`, where a second release fixes it up.
        atomic_json(&path, &operation)?;
        durable_rename(&path, &target)?;
    }
    lock.unlock()?;
    Ok(manifest)
}

fn write_manifest(root: &Path, manifest: &Manifest) -> Result<()> {
    if manifest.operations.is_empty() {
        return Ok(());
    }
    let name = format!(
        "{}-{}-{}.json",
        manifest.action,
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    );
    atomic_json(&root.join("memory/manifests").join(name), manifest)
}

fn print_manifest(manifest: &Manifest, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(manifest)?);
        return Ok(());
    }
    let verb = match (manifest.action, manifest.dry_run) {
        ("quarantine", true) => "would quarantine",
        ("quarantine", false) => "quarantined",
        _ => "released",
    };
    for entry in &manifest.operations {
        let detail = match (entry.stale_seconds, entry.last_receipt_state.as_deref()) {
            (Some(seconds), state) => format!(
                " (no receipt progress for {}h, last state {})",
                seconds / 3_600,
                state.unwrap_or("unknown")
            ),
            (None, _) => String::new(),
        };
        println!("{verb} {} -> {}{detail}", entry.operation_id, entry.to);
    }
    println!("{verb}: {} operation(s)", manifest.operations.len());
    Ok(())
}

fn read_operation(path: &Path) -> Result<MemoryOperation> {
    let operation: MemoryOperation = serde_json::from_slice(&fs::read(path)?)?;
    normalize_operation(operation)
}

fn normalize_operation(mut operation: MemoryOperation) -> Result<MemoryOperation> {
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
        // Already-normalized payloads pass through unchanged: their stored
        // payload hash (and any server receipt) is bound to these exact bytes.
        // New operations get non-null scope keys at the source (memory bridge).
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
                "agent_id": scope_key(arguments, "agent_id", "@project"),
                "user_id": scope_key(arguments, "user_id", &fallback_project_id())
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
    for state in [
        "submitting",
        "accepted",
        "pending",
        "completed",
        "rejected",
        "stalled",
    ] {
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

/// Flush a directory entry to disk so a just-completed rename survives a crash.
///
/// This is a POSIX idiom. On Windows `File::open` on a directory fails with "Access is denied"
/// (os error 5) — opening a directory there needs `FILE_FLAG_BACKUP_SEMANTICS` — and there is no
/// directory-fsync equivalent to call anyway; NTFS journals directory metadata itself. So the
/// non-unix twin is a no-op rather than an error that fails every snapshot commit.
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn print_status(root: &Path, stale_after: Duration, json_output: bool) -> Result<()> {
    let health = accepted_health(root, Utc::now(), stale_after)?;
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
        memory_stalled: json_files(&root.join("memory/stalled"))?.len(),
        memory_stale: health.stale,
        oldest_accepted_age_seconds: health.oldest_age_seconds,
        stale_after_seconds: stale_after.as_secs(),
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
        println!(
            "memory stale: {} (no receipt progress for {}s)",
            status.memory_stale, status.stale_after_seconds
        );
        println!(
            "oldest accepted age: {}",
            status
                .oldest_accepted_age_seconds
                .map_or_else(|| "-".to_owned(), |seconds| format!("{seconds}s"))
        );
        println!("memory stalled: {}", status.memory_stalled);
        println!("memory completed: {}", status.memory_completed);
        println!("memory rejected: {}", status.memory_rejected);
        println!("ambiguous delivery: {}", status.ambiguous_delivery);
    }
    Ok(())
}

fn scope_key(arguments: &Value, key: &str, fallback: &str) -> Value {
    match arguments.get(key).and_then(Value::as_str).map(str::trim) {
        Some(value) if !value.is_empty() => Value::String(value.to_owned()),
        _ => Value::String(fallback.to_owned()),
    }
}

/// Project scope for a legacy bridge operation that did not name one: the
/// explicit environment id, else an explicit `project:unknown` sentinel that the
/// surreal-memory re-key operation can repair (never a null key).
fn fallback_project_id() -> String {
    std::env::var("PROMETHEUS_PROJECT_ID")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "project:unknown".to_owned())
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
            first_accepted_at: None,
            last_receipt_state: None,
            last_receipt_progress_seq: None,
            last_receipt_change_at: None,
            unchanged_polls: 0,
            next_poll_at: None,
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
        let outcome =
            apply_receipt(temp.path(), &path, &mut operation, receipt, Utc::now()).unwrap();
        assert_eq!(outcome, ReceiptOutcome::Transitioned);
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

        let error = apply_receipt(temp.path(), &path, &mut operation, mismatched, Utc::now())
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

    #[cfg(unix)]
    #[test]
    fn normalized_submitting_operation_is_not_rewritten_during_recovery() {
        use std::os::unix::fs::MetadataExt;

        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"existing record"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        operation.state = "submitting".to_owned();
        let path = temp.path().join("memory/submitting/operation-1.json");
        atomic_json(&path, &operation).unwrap();
        let inode_before = fs::metadata(&path).unwrap().ino();

        recover_memory_submitting(temp.path()).unwrap();

        assert_eq!(fs::metadata(&path).unwrap().ino(), inode_before);
        assert_eq!(read_operation(&path).unwrap(), operation);
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

        reconcile_memory(
            temp.path(),
            &path,
            &url,
            &reqwest::Client::new(),
            Utc::now(),
        )
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
    async fn ledger_lookup_timeout_keeps_the_operation_durable() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        operation.state = "submitting".to_owned();
        let path = temp.path().join("memory/submitting/operation-1.json");
        atomic_json(&path, &operation).unwrap();

        let app = Router::new().route(
            "/api/v2/operations/{operation_id}",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                StatusCode::NOT_FOUND
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = memory_client(Duration::from_millis(25)).unwrap();
        let started = Instant::now();

        let error = reconcile_memory(
            temp.path(),
            &path,
            &format!("http://{address}"),
            &client,
            Utc::now(),
        )
        .await
        .unwrap_err();

        assert!(memory_transport_unavailable(&error));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(path.exists());
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

        let result = reconcile_memory(
            temp.path(),
            &path,
            &url,
            &reqwest::Client::new(),
            Utc::now(),
        )
        .await;

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

        reconcile_memory(
            temp.path(),
            &path,
            &url,
            &reqwest::Client::new(),
            Utc::now(),
        )
        .await
        .unwrap();

        assert_eq!(*posts.lock().unwrap(), 1);
        let accepted = temp.path().join("memory/accepted/operation-1.json");
        assert!(accepted.exists());
        assert_eq!(read_operation(&accepted).unwrap().state, "accepted");
        server.abort();
    }

    #[test]
    fn receipt_progress_resets_backoff_and_the_staleness_clock() {
        let start = parse_time("2026-10-01T00:00:00Z").unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let mut planned = receipt(&operation, "planned");
        planned.updated_at = start.to_rfc3339();

        assert!(observe_receipt(&mut operation, &planned, start));
        assert!(!observe_receipt(
            &mut operation,
            &planned,
            start + TimeDelta::minutes(1)
        ));
        assert!(!observe_receipt(
            &mut operation,
            &planned,
            start + TimeDelta::minutes(2)
        ));
        assert_eq!(operation.unchanged_polls, 2);
        assert_eq!(
            operation.next_poll_at.as_deref().and_then(parse_time),
            Some(start + TimeDelta::minutes(4))
        );
        let later = start + TimeDelta::hours(7);
        assert!(is_stale(&operation, later, Duration::from_secs(6 * 3_600)));

        planned.progress_seq += 1;
        assert!(observe_receipt(&mut operation, &planned, later));
        assert_eq!(operation.unchanged_polls, 0);
        assert!(operation.next_poll_at.is_none());
        assert!(!is_stale(&operation, later, Duration::from_secs(6 * 3_600)));
        assert_eq!(poll_backoff(40), TimeDelta::seconds(3_600));
    }

    #[test]
    fn released_legacy_record_gets_a_full_staleness_window() {
        let temp = TempDir::new().unwrap();
        ensure_layout(temp.path()).unwrap();
        let mut operation = operation("add_memory", json!({"content":"delta"}));
        operation.payload_hash = Some(canonical_payload_hash(&operation.arguments).unwrap());
        let mut planned = receipt(&operation, "planned");
        planned.updated_at = "2026-09-21T00:00:00Z".to_owned();
        operation.receipt = Some(planned.clone());
        operation.state = "stalled".to_owned();
        atomic_json(
            &temp.path().join("memory/stalled/operation-1.json"),
            &operation,
        )
        .unwrap();
        let now = Utc::now();

        release(temp.path(), None, now).unwrap();
        let path = temp.path().join("memory/accepted/operation-1.json");
        let mut released = read_operation(&path).unwrap();
        observe_receipt(&mut released, &planned, now + TimeDelta::minutes(1));

        assert!(!is_stale(
            &released,
            now + TimeDelta::minutes(1),
            Duration::from_secs(6 * 3_600)
        ));
    }

    #[test]
    fn duration_flags_require_a_unit() {
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("15m"), Ok(Duration::from_secs(900)));
        assert_eq!(parse_duration("6h"), Ok(Duration::from_secs(21_600)));
        assert_eq!(parse_duration("2d"), Ok(Duration::from_secs(172_800)));
        for invalid in ["", "6", "h", "6x", "-1h", "1.5h", "0s", "0h"] {
            assert!(parse_duration(invalid).is_err(), "{invalid:?}");
        }
    }

    /// Revert line: delete the `.filter(|path| !path.starts_with(...))` call in
    /// `git_changed_paths` to make this test fail.
    #[test]
    fn git_changed_paths_excludes_the_collectors_own_wiki_writes() {
        // ~keep The worker writes session records into `.prometheus/knowledge`,
        // so an unfiltered `git status` reported them as the next turn's changed
        // work — records differed only by a growing list of previously-generated
        // karpathy files.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(root)
                .status()
                .expect("git setup");
        }
        fs::create_dir_all(root.join(".prometheus/knowledge/wiki")).expect("mkdir wiki");
        fs::write(
            root.join(".prometheus/knowledge/wiki/karpathy-session-x.md"),
            "x",
        )
        .expect("w1");
        fs::write(root.join("real_change.rs"), "fn main() {}").expect("w2");
        // Commit-then-modify so git reports the wiki as a full tracked path.
        // (Left untracked, git collapses the whole tree to a bare `.prometheus/`
        // entry, which is the other form the filter must also exclude.)
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(root)
            .status()
            .expect("git add");
        Command::new("git")
            .args(["commit", "--quiet", "-m", "seed"])
            .current_dir(root)
            .status()
            .expect("git commit");
        fs::write(
            root.join(".prometheus/knowledge/wiki/karpathy-session-x.md"),
            "y",
        )
        .expect("w3");
        fs::write(root.join("real_change.rs"), "fn main() { }").expect("w4");

        let paths = git_changed_paths(root);
        assert!(
            paths.iter().any(|p| p.contains("real_change.rs")),
            "genuine work must still be reported, got {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains(".prometheus/knowledge/")),
            "the collector must not report its own writes, got {paths:?}"
        );
    }
}
