//! `pk candidates list|accept|reject` — the human confirmation step for
//! promotion candidates proposed by the learning worker (design:
//! team-aware-learning-memory §3). Skill candidates share the directory
//! layout; accepting them is not implemented yet.

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use pk_core::{paths::home_dir, ArticleId, WikiEntry};
use pk_store::{commit_prompt_snapshot, MarkdownStore};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MEMORY_STATES: &[&str] = &[
    "pending",
    "submitting",
    "accepted",
    "retry",
    "completed",
    "rejected",
    "dead-letter",
    "stalled",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CandidateKind {
    /// Lessons proposed for user/global scope (`~/.prometheus/promotion-candidates`).
    Promotion,
    /// Proposed skills (`~/.prometheus/skill-candidates`).
    Skill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CandidateState {
    Pending,
    Accepted,
    Rejected,
}

impl CandidateState {
    fn dir(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum CandidatesCmd {
    /// List candidates (pending by default).
    List {
        #[arg(long, value_enum, default_value = "pending")]
        state: CandidateState,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Accept a pending candidate: write it to the shared KB, publish the shared
    /// prompt snapshot, queue a surreal-memory operation, move it to accepted/.
    Accept {
        #[arg()]
        id: String,
    },
    /// Reject a pending candidate: move it to rejected/. It is never proposed again.
    Reject {
        #[arg()]
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

pub async fn run(kind: CandidateKind, action: CandidatesCmd) -> Result<()> {
    let home = home_dir().context("HOME unavailable")?;
    let root = candidates_root(&home, kind);
    match action {
        CandidatesCmd::List { state, json } => list(&root, state, json),
        CandidatesCmd::Accept { id } => match kind {
            CandidateKind::Promotion => accept_promotion(&home, &root, &id).await,
            CandidateKind::Skill => bail!(
                "accepting skill candidates is not yet supported; use `pk candidates reject --kind skill` or wait for skill promotion support"
            ),
        },
        CandidatesCmd::Reject { id, reason } => reject(&root, &id, reason.as_deref()),
    }
}

fn candidates_root(home: &Path, kind: CandidateKind) -> PathBuf {
    home.join(".prometheus").join(match kind {
        CandidateKind::Promotion => "promotion-candidates",
        CandidateKind::Skill => "skill-candidates",
    })
}

fn list(root: &Path, state: CandidateState, json_output: bool) -> Result<()> {
    let directory = root.join(state.dir());
    let mut paths: Vec<PathBuf> = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    paths.sort();
    let mut candidates = Vec::new();
    for path in paths {
        let mut value: Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("unreadable candidate {}", path.display()))?;
        if value.get("id").and_then(Value::as_str).is_none() {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                value["id"] = Value::String(stem.to_owned());
            }
        }
        candidates.push(value);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&candidates)?);
        return Ok(());
    }
    if candidates.is_empty() {
        println!("no {} candidates", state.dir());
        return Ok(());
    }
    for candidate in &candidates {
        let text = |key: &str| candidate.get(key).and_then(Value::as_str).unwrap_or("-");
        let reasons = candidate
            .get("reasons")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "-".to_owned());
        let evidence = candidate
            .get("evidence")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let title = candidate
            .get("title")
            .or_else(|| candidate.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        println!(
            "{}\t{}\t{}\tevidence={}\t{}",
            text("id"),
            text("scope"),
            reasons,
            evidence,
            title
        );
    }
    Ok(())
}

/// Candidate ids become file names, so only a plain file-name alphabet is accepted.
fn validate_id(id: &str) -> Result<()> {
    let safe = !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !safe {
        bail!("invalid candidate id {id:?}");
    }
    Ok(())
}

fn pending_candidate(root: &Path, id: &str) -> Result<(PathBuf, Value)> {
    validate_id(id)?;
    let filename = format!("{id}.json");
    let pending = root.join("pending").join(&filename);
    if !pending.exists() {
        for decided in ["accepted", "rejected"] {
            if root.join(decided).join(&filename).exists() {
                bail!("candidate {id} is already {decided}");
            }
        }
        bail!("no pending candidate {id} under {}", root.display());
    }
    let value = serde_json::from_slice(&fs::read(&pending)?)
        .with_context(|| format!("unreadable candidate {}", pending.display()))?;
    Ok((pending, value))
}

async fn accept_promotion(home: &Path, root: &Path, id: &str) -> Result<()> {
    let (pending, mut candidate) = pending_candidate(root, id)?;
    let content = candidate
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .context("candidate has no content")?
        .to_owned();
    let title = candidate
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Promoted lesson")
        .to_owned();
    let scope = match candidate.get("scope").and_then(Value::as_str) {
        Some("user") => "user",
        _ => "global",
    };

    let entry_id = upsert_shared(home, id, &title, &content, scope, &candidate).await?;
    let operation_id = queue_memory(home, id, &content, scope)?;

    let now = rfc3339_now();
    candidate["state"] = json!("accepted");
    candidate["acceptedAt"] = json!(now);
    candidate["updatedAt"] = json!(now);
    candidate["sharedEntryId"] = json!(entry_id);
    candidate["memoryOperationId"] = json!(operation_id);
    atomic_json(&pending, &candidate)?;
    let accepted = root.join("accepted").join(format!("{id}.json"));
    durable_rename(&pending, &accepted)?;
    println!("accepted {id} → shared entry {entry_id}, memory operation {operation_id}");
    Ok(())
}

/// Write the lesson into `~/.prometheus/knowledge/shared` and publish the shared
/// prompt snapshot that `pk context` reads. Idempotent on retry.
async fn upsert_shared(
    home: &Path,
    id: &str,
    title: &str,
    content: &str,
    scope: &str,
    candidate: &Value,
) -> Result<String> {
    let shared = home.join(".prometheus/knowledge/shared");
    let store = MarkdownStore::open(&shared).await?;
    let entry_id = format!("promoted-{id}");
    let article_id = ArticleId::from(entry_id.clone());
    let unchanged = store
        .get(&article_id)
        .await
        .is_ok_and(|existing| existing.content == content);
    if !unchanged {
        let mut entry = WikiEntry::new(title.to_owned(), content.to_owned());
        entry.id = article_id.clone();
        entry.entry_type = Some("Lesson".to_owned());
        let mut tags: Vec<String> = candidate
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        for tag in ["promoted".to_owned(), format!("scope:{scope}")] {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        entry.tags = tags;
        entry.sources = candidate
            .get("evidence")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let project = item.get("projectId").and_then(Value::as_str)?;
                        let source = item.get("source").and_then(Value::as_str)?;
                        Some(format!("{project}:{source}").into())
                    })
                    .collect()
            })
            .unwrap_or_default();
        store.upsert(entry).await?;
        store.regenerate_index().await?;
        store.append_log("Promote", title, &article_id).await?;
    }
    commit_prompt_snapshot(&shared, "shared", store.snapshot().await?)?;
    Ok(entry_id)
}

/// Queue an `add_memory` operation in the learning-queue outbox, in the
/// worker's schemaVersion 2 format: `payloadHash` is the SHA-256 of the
/// serialized `arguments`, which the worker re-checks before submitting.
fn queue_memory(home: &Path, id: &str, content: &str, scope: &str) -> Result<String> {
    let queue = std::env::var_os("PROMETHEUS_LEARNING_QUEUE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".prometheus/learning-queue"));
    let operation_id = hex_digest(format!("promotion-candidate:{id}").as_bytes());
    let filename = format!("{operation_id}.json");
    if MEMORY_STATES
        .iter()
        .any(|state| queue.join("memory").join(state).join(&filename).exists())
    {
        return Ok(operation_id);
    }
    let (user_id, agent_id) = match scope {
        "user" => (resolve_user_scope(), "@user".to_owned()),
        _ => ("@global".to_owned(), "@global".to_owned()),
    };
    let arguments = json!({
        "content": content,
        "user_id": user_id,
        "agent_id": agent_id,
        "session_id": format!("promotion:{id}"),
        "categories": ["promotion", format!("scope:{scope}")]
    });
    let payload_hash = hex_digest(&serde_json::to_vec(&arguments)?);
    let operation = json!({
        "schemaVersion": 2,
        "operationId": operation_id,
        "method": "add_memory",
        "arguments": arguments,
        "dependencies": [],
        "payloadHash": payload_hash,
        "state": "pending",
        "queuedAt": rfc3339_now(),
        "lastError": null,
        "receipt": null
    });
    let pending = queue.join("memory/pending");
    fs::create_dir_all(&pending)?;
    harden_directory(&pending)?;
    atomic_json(&pending.join(&filename), &operation)?;
    Ok(operation_id)
}

/// `@user:<hash>`: `PROMETHEUS_USER_ID` when set, else the first 16 hex digits
/// of SHA-256 over the lowercased global git email, matching the skill
/// system's `project_id.py` resolver.
fn resolve_user_scope() -> String {
    if let Ok(value) = std::env::var("PROMETHEUS_USER_ID") {
        let value = value.trim();
        if !value.is_empty() {
            let bare = value
                .strip_prefix("@user:")
                .or_else(|| value.strip_prefix("user:"))
                .unwrap_or(value);
            return format!("@user:{bare}");
        }
    }
    let email = Command::new("git")
        .args(["config", "--global", "user.email"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase()
        })
        .filter(|email| !email.is_empty());
    match email {
        Some(email) => format!("@user:{}", &hex_digest(email.as_bytes())[..16]),
        None => "@user:unknown".to_owned(),
    }
}

fn reject(root: &Path, id: &str, reason: Option<&str>) -> Result<()> {
    let (pending, mut candidate) = pending_candidate(root, id)?;
    let now = rfc3339_now();
    candidate["state"] = json!("rejected");
    candidate["rejectedAt"] = json!(now);
    candidate["updatedAt"] = json!(now);
    if let Some(reason) = reason {
        candidate["rejectionReason"] = json!(reason);
    }
    atomic_json(&pending, &candidate)?;
    durable_rename(&pending, &root.join("rejected").join(format!("{id}.json")))?;
    println!("rejected {id}");
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn atomic_json(path: &Path, value: &Value) -> Result<()> {
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
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    sync_directory(parent)
}

fn durable_rename(source: &Path, target: &Path) -> Result<()> {
    let target_parent = target.parent().context("target path has no parent")?;
    fs::create_dir_all(target_parent)?;
    fs::rename(source, target)?;
    sync_directory(target_parent)?;
    if let Some(source_parent) = source.parent().filter(|parent| *parent != target_parent) {
        sync_directory(source_parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
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

/// UTC now as RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`). pk-cli has no chrono
/// dependency, and adding one would change Cargo.lock.
fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs()) as i64;
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant), proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}
