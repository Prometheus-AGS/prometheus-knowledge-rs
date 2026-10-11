//! Skill discovery (design: team-aware-learning-memory §6).
//!
//! After each `run-once`, every processed session is fingerprinted into
//! `~/.prometheus/learning-index/workflows.jsonl`, and two kinds of candidate
//! are proposed under `~/.prometheus/skill-candidates/pending/<id>.json`:
//!
//! - **new-skill**: a workflow fingerprint (prompt word-trigrams plus
//!   tool-sequence 3-grams) seen in at least [`MIN_SESSIONS`] sessions or
//!   [`MIN_PROJECTS`] projects that no existing skill covers. A skill covers a
//!   workflow when it was invoked in at least half of the sessions that show it.
//! - **skill-update**: a skill that users corrected at least
//!   [`MIN_CORRECTIONS`] times after it ran, attributed to the team role that
//!   ran it. It is also logged in the format `propose-skill-update.sh` uses
//!   (`~/.prometheus/skill-updates/`), so `/pmpo-skill-creator --update` finds it.
//!
//! Nothing here creates or edits a skill. A human accepts a candidate with
//! `pk candidates accept --kind skill`, which only prints the invocation.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::{
    atomic_json, harden_directory,
    transcript::{SkillUse, TranscriptDigest},
};

pub(crate) const MIN_SESSIONS: usize = 3;
pub(crate) const MIN_PROJECTS: usize = 2;
pub(crate) const MIN_CORRECTIONS: usize = 2;
/// Blend of prompt-shingle and tool-shingle Jaccard needed to call two
/// sessions the same workflow.
pub(crate) const WORKFLOW_SIMILARITY: f64 = 0.5;
const SHINGLE_SIZE: usize = 3;
/// A session needs this many tool steps to have a tool-sequence 3-gram.
const MIN_TOOL_STEPS: usize = 3;
const FINGERPRINT_PROMPTS: usize = 5;
const SUMMARY_CHARS: usize = 160;
const MAX_EVIDENCE_ARTIFACTS: usize = 10;

/// What one processed job tells skill discovery.
#[derive(Debug, Clone)]
pub(crate) struct SessionObservation {
    pub project_id: String,
    pub project_root: PathBuf,
    pub session_id: String,
    pub team_id: Option<String>,
    pub role_id: Option<String>,
    pub digest: TranscriptDigest,
}

/// One line of `workflows.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRecord {
    schema_version: u32,
    workflow_id: String,
    project_id: String,
    project_root: PathBuf,
    session_id: String,
    recorded_at: String,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    role_id: Option<String>,
    summary: String,
    #[serde(default)]
    artifacts: Vec<String>,
    #[serde(default)]
    skill_uses: Vec<SkillUse>,
    /// Sorted, de-duplicated word-trigram hashes of the user prompts.
    prompt_shingles: Vec<u64>,
    /// Sorted, de-duplicated 3-gram hashes of the tool sequence.
    tool_shingles: Vec<u64>,
}

impl WorkflowRecord {
    fn fingerprintable(&self) -> bool {
        !self.prompt_shingles.is_empty() && !self.tool_shingles.is_empty()
    }

    fn skills(&self) -> BTreeSet<&str> {
        self.skill_uses.iter().map(|u| u.skill.as_str()).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    workflow_id: String,
    project_id: String,
    project_root: PathBuf,
    session_id: String,
    recorded_at: String,
    similarity: f64,
    skills: Vec<String>,
    artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    team_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    corrections: Vec<String>,
}

pub(crate) fn observation_for(
    project_id: &str,
    project_root: &Path,
    session_id: &str,
    team_id: Option<&str>,
    role_id: Option<&str>,
    digest: TranscriptDigest,
) -> SessionObservation {
    let present = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    SessionObservation {
        project_id: project_id.to_owned(),
        project_root: project_root.to_path_buf(),
        session_id: session_id.to_owned(),
        team_id: present(team_id),
        role_id: present(role_id),
        digest,
    }
}

/// Fingerprint new sessions, then propose candidates from the whole index.
pub(crate) fn run(sessions: &[SessionObservation]) -> Result<usize> {
    let home = pk_core::paths::home_dir().context("HOME unavailable")?;
    run_in(&home.join(".prometheus"), sessions)
}

fn run_in(prometheus_home: &Path, sessions: &[SessionObservation]) -> Result<usize> {
    let index_path = prometheus_home.join("learning-index/workflows.jsonl");
    let mut records = read_index(&index_path)?;
    append_new(&index_path, &mut records, sessions)?;
    propose(prometheus_home, &records)
}

fn read_index(path: &Path) -> Result<Vec<WorkflowRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for line in BufReader::new(File::open(path)?)
        .lines()
        .map_while(std::result::Result::ok)
    {
        // A torn final line from a crash mid-append is skipped, not fatal.
        if let Ok(record) = serde_json::from_str::<WorkflowRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn append_new(
    path: &Path,
    records: &mut Vec<WorkflowRecord>,
    sessions: &[SessionObservation],
) -> Result<()> {
    let mut known: HashSet<String> = records.iter().map(|r| r.workflow_id.clone()).collect();
    let now = Utc::now().to_rfc3339();
    let mut fresh = Vec::new();
    for session in sessions {
        let digest = &session.digest;
        if digest.prompts.is_empty() && digest.skill_uses.is_empty() {
            continue;
        }
        let workflow_id =
            hex_digest(&format!("{}\0{}", session.project_id, session.session_id))[..32].to_owned();
        if !known.insert(workflow_id.clone()) {
            continue;
        }
        let prompt_text = digest
            .prompts
            .iter()
            .take(FINGERPRINT_PROMPTS)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        fresh.push(WorkflowRecord {
            schema_version: 1,
            workflow_id,
            project_id: session.project_id.clone(),
            project_root: session.project_root.clone(),
            session_id: session.session_id.clone(),
            recorded_at: now.clone(),
            team_id: session.team_id.clone(),
            role_id: session.role_id.clone(),
            summary: summary_for(digest.prompts.first().map_or("", String::as_str)),
            artifacts: digest
                .artifacts
                .iter()
                .take(MAX_EVIDENCE_ARTIFACTS)
                .cloned()
                .collect(),
            skill_uses: digest.skill_uses.clone(),
            prompt_shingles: shingles(&tokens(&prompt_text)),
            tool_shingles: if digest.tool_sequence.len() >= MIN_TOOL_STEPS {
                shingles(&digest.tool_sequence)
            } else {
                Vec::new()
            },
        });
    }
    if fresh.is_empty() {
        return Ok(());
    }
    let parent = path.parent().context("index path has no parent")?;
    fs::create_dir_all(parent)?;
    harden_directory(parent)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    for record in &fresh {
        serde_json::to_writer(&mut file, record)?;
        writeln!(file)?;
    }
    file.sync_data()?;
    records.extend(fresh);
    Ok(())
}

fn propose(prometheus_home: &Path, records: &[WorkflowRecord]) -> Result<usize> {
    let root = prometheus_home.join("skill-candidates");
    for state in ["pending", "accepted", "rejected"] {
        let directory = root.join(state);
        fs::create_dir_all(&directory)?;
        harden_directory(&directory)?;
    }
    harden_directory(&root)?;

    let mut ordered: Vec<&WorkflowRecord> = records.iter().collect();
    ordered.sort_by(|a, b| {
        a.recorded_at
            .cmp(&b.recorded_at)
            .then_with(|| a.workflow_id.cmp(&b.workflow_id))
    });

    let mut written = 0;
    for candidate in new_skill_candidates(&ordered)
        .into_iter()
        .chain(update_candidates(&ordered))
    {
        let update = candidate.get("candidateType").and_then(Value::as_str) == Some("skill-update");
        let changed = write_candidate(&root, candidate.clone())?;
        if changed {
            written += 1;
            if update {
                log_update_proposal(prometheus_home, &candidate);
            }
        }
    }
    Ok(written)
}

/// Greedy single-pass clustering in record order, so a cluster's anchor (and
/// therefore its candidate id) never changes as the index grows.
fn new_skill_candidates(ordered: &[&WorkflowRecord]) -> Vec<Value> {
    let mut clusters: Vec<Vec<(&WorkflowRecord, f64)>> = Vec::new();
    for record in ordered.iter().copied().filter(|r| r.fingerprintable()) {
        let joined = clusters.iter_mut().find_map(|cluster| {
            let similarity = workflow_similarity(cluster[0].0, record);
            (similarity >= WORKFLOW_SIMILARITY).then_some((cluster, similarity))
        });
        match joined {
            Some((cluster, similarity)) => cluster.push((record, similarity)),
            None => clusters.push(vec![(record, 1.0)]),
        }
    }
    clusters
        .iter()
        .filter_map(|c| new_skill_candidate(c))
        .collect()
}

fn new_skill_candidate(cluster: &[(&WorkflowRecord, f64)]) -> Option<Value> {
    let sessions = cluster.len();
    let projects: BTreeSet<&str> = cluster.iter().map(|(r, _)| r.project_id.as_str()).collect();
    let mut reasons = Vec::new();
    if sessions >= MIN_SESSIONS {
        reasons.push(format!("sessions>={MIN_SESSIONS}"));
    }
    if projects.len() >= MIN_PROJECTS {
        reasons.push(format!("projects>={MIN_PROJECTS}"));
    }
    if reasons.is_empty() {
        return None;
    }
    let mut invoked: BTreeMap<&str, usize> = BTreeMap::new();
    for (record, _) in cluster {
        for skill in record.skills() {
            *invoked.entry(skill).or_default() += 1;
        }
    }
    // A skill used in at least half of the sessions already covers the workflow.
    if invoked.values().any(|count| count * 2 >= sessions) {
        return None;
    }
    reasons.push("no-covering-skill".to_owned());

    let anchor = cluster[0].0;
    let now = Utc::now().to_rfc3339();
    Some(json!({
        "schemaVersion": 1,
        "id": format!("skill-new-{}", &hex_digest(&anchor.workflow_id)[..16]),
        "kind": "skill",
        "candidateType": "new-skill",
        "state": "pending",
        "reasons": reasons,
        "title": format!("New skill: {}", anchor.summary),
        "summary": anchor.summary,
        "sessionCount": sessions,
        "projectCount": projects.len(),
        "invokedSkills": invoked.keys().collect::<Vec<_>>(),
        "evidence": cluster.iter().map(|(r, similarity)| evidence_for(r, *similarity, &[])).collect::<Vec<_>>(),
        "createdAt": now,
        "updatedAt": now,
    }))
}

/// One candidate per (skill, team, role) that users corrected at least
/// [`MIN_CORRECTIONS`] times after it ran; evidence is one entry per session.
fn update_candidates(ordered: &[&WorkflowRecord]) -> Vec<Value> {
    type Key = (String, Option<String>, Option<String>);
    let mut groups: BTreeMap<Key, Vec<(&WorkflowRecord, Vec<String>)>> = BTreeMap::new();
    for record in ordered.iter().copied() {
        let mut per_skill: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for used in &record.skill_uses {
            per_skill
                .entry(used.skill.as_str())
                .or_default()
                .extend(used.corrections.iter().cloned());
        }
        for (skill, corrections) in per_skill {
            if corrections.len() >= MIN_CORRECTIONS {
                groups
                    .entry((
                        skill.to_owned(),
                        record.team_id.clone(),
                        record.role_id.clone(),
                    ))
                    .or_default()
                    .push((record, corrections));
            }
        }
    }
    groups
        .into_iter()
        .map(|((skill, team_id, role_id), sessions)| {
            let role = role_label(team_id.as_deref(), role_id.as_deref());
            let total: usize = sessions.iter().map(|(_, c)| c.len()).sum();
            let now = Utc::now().to_rfc3339();
            let key = format!(
                "{skill}\0{}\0{}",
                team_id.as_deref().unwrap_or(""),
                role_id.as_deref().unwrap_or("")
            );
            json!({
                "schemaVersion": 1,
                "id": format!("skill-upd-{}", &hex_digest(&key)[..16]),
                "kind": "skill",
                "candidateType": "skill-update",
                "state": "pending",
                "skillName": skill,
                "teamId": team_id,
                "roleId": role_id,
                "reasons": ["post-skill-corrections"],
                "title": format!(
                    "Update {skill}: {total} correction(s) after use by {}",
                    role.as_deref().unwrap_or("an unattributed agent")
                ),
                "evidence": sessions
                    .iter()
                    .map(|(r, corrections)| evidence_for(r, 1.0, corrections))
                    .collect::<Vec<_>>(),
                "createdAt": now,
                "updatedAt": now,
            })
        })
        .collect()
}

fn evidence_for(record: &WorkflowRecord, similarity: f64, corrections: &[String]) -> Value {
    serde_json::to_value(Evidence {
        workflow_id: record.workflow_id.clone(),
        project_id: record.project_id.clone(),
        project_root: record.project_root.clone(),
        session_id: record.session_id.clone(),
        recorded_at: record.recorded_at.clone(),
        similarity: (similarity * 1000.0).round() / 1000.0,
        skills: record.skills().into_iter().map(str::to_owned).collect(),
        artifacts: record.artifacts.clone(),
        team_id: record.team_id.clone(),
        role_id: record.role_id.clone(),
        corrections: corrections.to_vec(),
    })
    .unwrap_or(Value::Null)
}

/// Write a candidate unless it was already decided or its evidence is
/// unchanged. Returns whether a file was written.
fn write_candidate(root: &Path, mut candidate: Value) -> Result<bool> {
    let id = candidate["id"].as_str().context("candidate has no id")?;
    let filename = format!("{id}.json");
    if root.join("accepted").join(&filename).exists()
        || root.join("rejected").join(&filename).exists()
    {
        return Ok(false);
    }
    let pending = root.join("pending").join(&filename);
    if pending.exists() {
        let existing: Value = serde_json::from_slice(&fs::read(&pending)?)?;
        if evidence_ids(&existing) == evidence_ids(&candidate) {
            return Ok(false);
        }
        if let Some(created_at) = existing["createdAt"].as_str() {
            candidate["createdAt"] = json!(created_at);
        }
    }
    atomic_json(&pending, &candidate)?;
    Ok(true)
}

fn evidence_ids(candidate: &Value) -> BTreeSet<String> {
    candidate["evidence"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["workflowId"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Record an update proposal the way `propose-skill-update.sh` does: a
/// `<skill>::<date>` marker plus the `/pmpo-skill-creator --update` hint in
/// `pending.log`, and a placeholder `<skill>-<date>.diff`. The marker carries
/// the attributed role. Failures only warn: the candidate file is the record.
fn log_update_proposal(prometheus_home: &Path, candidate: &Value) {
    let Some(skill) = candidate["skillName"].as_str() else {
        return;
    };
    let safe = !skill.is_empty()
        && skill
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !safe {
        return;
    }
    let hits = evidence_ids(candidate).len();
    let role = role_label(candidate["teamId"].as_str(), candidate["roleId"].as_str());
    if let Err(error) = write_update_log(prometheus_home, skill, hits, role.as_deref()) {
        tracing::warn!(error = %error, skill, "skill update log failed");
    }
}

fn write_update_log(
    prometheus_home: &Path,
    skill: &str,
    hits: usize,
    role: Option<&str>,
) -> Result<()> {
    let directory = prometheus_home.join("skill-updates");
    fs::create_dir_all(&directory)?;
    harden_directory(&directory)?;
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let marker = format!("{skill}::{today}");
    let log_path = directory.join("pending.log");
    let existing = fs::read_to_string(&log_path).unwrap_or_default();
    if !existing.contains(&marker) {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let role = role.map(|r| format!(" role={r}")).unwrap_or_default();
        writeln!(log, "{marker} hits={hits}{role}")?;
        writeln!(
            log,
            "Run: /pmpo-skill-creator --update {skill}  # to review and optionally apply"
        )?;
    }
    let diff_path = directory.join(format!("{skill}-{today}.diff"));
    if !diff_path.exists() {
        fs::write(
            &diff_path,
            format!(
                "# Skill update candidate — {skill} ({today})\n\
                 # Sessions with post-skill corrections: {hits}\n\
                 # Run by: {}\n\
                 #\n\
                 # This file was created by the learning worker's skill discovery.\n\
                 # It does NOT contain the actual diff yet.\n\
                 #\n\
                 # To generate and review the proposed changes, run:\n\
                 #   /pmpo-skill-creator --update {skill}\n",
                role.unwrap_or("unattributed")
            ),
        )?;
    }
    Ok(())
}

fn role_label(team_id: Option<&str>, role_id: Option<&str>) -> Option<String> {
    match (team_id, role_id) {
        (Some(team), Some(role)) => Some(format!("{team}/{role}")),
        (None, Some(role)) => Some(role.to_owned()),
        (Some(team), None) => Some(team.to_owned()),
        (None, None) => None,
    }
}

fn workflow_similarity(left: &WorkflowRecord, right: &WorkflowRecord) -> f64 {
    0.5 * jaccard(&left.prompt_shingles, &right.prompt_shingles)
        + 0.5 * jaccard(&left.tool_shingles, &right.tool_shingles)
}

fn summary_for(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    let mut summary: String = first_line.chars().take(SUMMARY_CHARS).collect();
    if first_line.chars().count() > SUMMARY_CHARS {
        summary.push('…');
    }
    if summary.is_empty() {
        "Repeated workflow".to_owned()
    } else {
        summary
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn shingles(tokens: &[String]) -> Vec<u64> {
    if tokens.len() < SHINGLE_SIZE {
        return Vec::new();
    }
    let mut hashes: Vec<u64> = tokens
        .windows(SHINGLE_SIZE)
        .map(|window| shingle_hash(&window.join(" ")))
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    hashes
}

/// First 8 bytes of SHA-256: stable across Rust releases, unlike `DefaultHasher`.
fn shingle_hash(shingle: &str) -> u64 {
    let digest = Sha256::digest(shingle.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// Jaccard similarity of two sorted, de-duplicated sets.
fn jaccard(left: &[u64], right: &[u64]) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut shared) = (0, 0, 0usize);
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    shared as f64 / (left.len() + right.len() - shared) as f64
}

fn hex_digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
