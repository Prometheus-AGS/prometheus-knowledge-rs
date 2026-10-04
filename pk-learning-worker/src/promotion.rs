//! Promotion detector (design: team-aware-learning-memory §3).
//!
//! After each `run-once`, lessons seen by the worker are fingerprinted into
//! `~/.prometheus/learning-index/lessons.jsonl`, and lessons that look reusable
//! beyond their project become promotion candidates under
//! `~/.prometheus/promotion-candidates/pending/<id>.json`. A human accepts or
//! rejects a candidate with `pk candidates`; nothing here writes a shared scope.
//!
//! A lesson becomes a candidate when any of these holds:
//! - it recurs (word-trigram Jaccard >= 0.6) in at least two distinct projects;
//! - it is tagged global but carries no `[GLOBAL]` marker (marker lines are
//!   promoted immediately by another path, so they are never proposed);
//! - it names a dependency or CLI and contains no repo-relative path.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::{atomic_json, harden_directory};

pub(crate) const RECURRENCE_THRESHOLD: f64 = 0.6;
const SHINGLE_SIZE: usize = 3;
const LESSON_TEXT_LIMIT: usize = 2_000;
const LESSON_PREFIXES: &[&str] = &["lesson:", "gotcha:", "pattern:", "learned:"];
const IMMEDIATE_MARKERS: &[&str] = &["[GLOBAL]", "[USER]"];
const GLOBAL_TAGS: &[&str] = &["global", "scope:global", "vis:global"];
const USER_TAGS: &[&str] = &["user", "scope:user", "vis:user"];
/// Command-line tools whose name inside a code span counts as "names a CLI".
const KNOWN_CLIS: &[&str] = &[
    "apt",
    "brew",
    "bun",
    "bunx",
    "cargo",
    "claude",
    "cmake",
    "codex",
    "curl",
    "deno",
    "docker",
    "gh",
    "git",
    "go",
    "gradle",
    "helm",
    "jq",
    "kubectl",
    "launchctl",
    "make",
    "mvn",
    "node",
    "npm",
    "npx",
    "pip",
    "pip3",
    "pk",
    "pnpm",
    "python",
    "python3",
    "rustc",
    "rustup",
    "sqlite3",
    "systemctl",
    "terraform",
    "uv",
    "uvx",
    "yarn",
];
/// First path segments that mark a repo-relative path even without an extension.
const REPO_DIRS: &[&str] = &[
    ".claude",
    ".github",
    ".kbd-orchestrator",
    ".prometheus",
    "crates",
    "docs",
    "hooks",
    "lib",
    "scripts",
    "shared",
    "skills",
    "src",
    "tests",
];

/// A lesson observed during this run, before fingerprinting.
#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub project_id: String,
    pub project_root: PathBuf,
    /// Where the lesson came from: `session:<id>#<n>` or `kb:<entry id>`.
    pub source: String,
    pub session_id: Option<String>,
    pub text: String,
    pub tags: Vec<String>,
}

/// One line of `lessons.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LessonRecord {
    schema_version: u32,
    lesson_id: String,
    project_id: String,
    project_root: PathBuf,
    source: String,
    #[serde(default)]
    session_id: Option<String>,
    recorded_at: String,
    text: String,
    #[serde(default)]
    tags: Vec<String>,
    /// Lesson carries an immediate-promotion marker; never proposed.
    #[serde(default)]
    marked: bool,
    /// Sorted, de-duplicated word-trigram hashes.
    shingles: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    lesson_id: String,
    project_id: String,
    project_root: PathBuf,
    source: String,
    #[serde(default)]
    session_id: Option<String>,
    recorded_at: String,
    similarity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    schema_version: u32,
    id: String,
    kind: String,
    state: String,
    /// `global` or `user`.
    scope: String,
    reasons: Vec<String>,
    title: String,
    content: String,
    tags: Vec<String>,
    fingerprint: String,
    evidence: Vec<Evidence>,
    created_at: String,
    updated_at: String,
}

/// Turn the lessons of one processed job into observations.
pub(crate) fn observations_from_message(
    project_id: &str,
    project_root: &Path,
    session_id: &str,
    message: &str,
) -> Vec<Observation> {
    extract_lessons(message)
        .into_iter()
        .enumerate()
        .map(|(index, text)| Observation {
            project_id: project_id.to_owned(),
            project_root: project_root.to_path_buf(),
            source: format!("session:{session_id}#{index}"),
            session_id: Some(session_id.to_owned()),
            tags: hashtags(&text),
            text,
        })
        .collect()
}

/// Typed lessons already in a project knowledge base (`Lesson`, `Gotcha`).
/// Their frontmatter tags are what makes "tagged global" observable.
pub(crate) fn observations_from_entries(
    project_id: &str,
    project_root: &Path,
    entries: &[pk_core::WikiEntry],
) -> Vec<Observation> {
    entries
        .iter()
        .filter(|entry| {
            entry.entry_type.as_deref().is_some_and(|kind| {
                kind.eq_ignore_ascii_case("lesson") || kind.eq_ignore_ascii_case("gotcha")
            })
        })
        .map(|entry| Observation {
            project_id: project_id.to_owned(),
            project_root: project_root.to_path_buf(),
            source: format!("kb:{}", entry.id.as_str()),
            session_id: None,
            text: entry
                .content
                .trim()
                .chars()
                .take(LESSON_TEXT_LIMIT)
                .collect(),
            tags: entry
                .tags
                .iter()
                .map(|tag| tag.to_ascii_lowercase())
                .collect(),
        })
        .collect()
}

/// Fingerprint new observations, then propose candidates from the whole index.
pub(crate) fn run(observations: &[Observation]) -> Result<usize> {
    let home = pk_core::paths::home_dir().context("HOME unavailable")?;
    run_in(&home.join(".prometheus"), observations)
}

fn run_in(prometheus_home: &Path, observations: &[Observation]) -> Result<usize> {
    let index_path = prometheus_home.join("learning-index/lessons.jsonl");
    let mut records = read_index(&index_path)?;
    append_new(&index_path, &mut records, observations)?;
    propose(&prometheus_home.join("promotion-candidates"), &records)
}

fn read_index(path: &Path) -> Result<Vec<LessonRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for line in BufReader::new(File::open(path)?)
        .lines()
        .map_while(std::result::Result::ok)
    {
        // A torn final line from a crash mid-append is skipped, not fatal.
        if let Ok(record) = serde_json::from_str::<LessonRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn append_new(
    path: &Path,
    records: &mut Vec<LessonRecord>,
    observations: &[Observation],
) -> Result<()> {
    let mut known: HashSet<String> = records.iter().map(|r| r.lesson_id.clone()).collect();
    let now = Utc::now().to_rfc3339();
    let mut fresh = Vec::new();
    for observation in observations {
        let tokens = tokens(&observation.text);
        if tokens.is_empty() {
            continue;
        }
        let lesson_id = hex_digest(&format!(
            "{}\0{}\0{}",
            observation.project_id,
            observation.source,
            tokens.join(" ")
        ))[..32]
            .to_owned();
        if !known.insert(lesson_id.clone()) {
            continue;
        }
        fresh.push(LessonRecord {
            schema_version: 1,
            lesson_id,
            project_id: observation.project_id.clone(),
            project_root: observation.project_root.clone(),
            source: observation.source.clone(),
            session_id: observation.session_id.clone(),
            recorded_at: now.clone(),
            text: observation.text.chars().take(LESSON_TEXT_LIMIT).collect(),
            tags: observation.tags.clone(),
            marked: IMMEDIATE_MARKERS
                .iter()
                .any(|marker| observation.text.contains(marker)),
            shingles: shingles(&tokens),
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

/// Greedy single-pass clustering in record order, so an existing cluster's
/// anchor (and therefore its candidate id) never changes as the index grows.
fn propose(candidates_root: &Path, records: &[LessonRecord]) -> Result<usize> {
    let mut ordered: Vec<&LessonRecord> = records.iter().filter(|r| !r.marked).collect();
    ordered.sort_by(|a, b| {
        a.recorded_at
            .cmp(&b.recorded_at)
            .then_with(|| a.lesson_id.cmp(&b.lesson_id))
    });
    let mut clusters: Vec<Vec<(&LessonRecord, f64)>> = Vec::new();
    for record in ordered {
        let joined = clusters.iter_mut().find_map(|cluster| {
            let similarity = jaccard(&cluster[0].0.shingles, &record.shingles);
            (similarity >= RECURRENCE_THRESHOLD).then_some((cluster, similarity))
        });
        match joined {
            Some((cluster, similarity)) => cluster.push((record, similarity)),
            None => clusters.push(vec![(record, 1.0)]),
        }
    }

    for state in ["pending", "accepted", "rejected"] {
        let directory = candidates_root.join(state);
        fs::create_dir_all(&directory)?;
        harden_directory(&directory)?;
    }
    harden_directory(candidates_root)?;

    let mut written = 0;
    for cluster in &clusters {
        if let Some(candidate) = candidate_for(cluster) {
            if write_candidate(candidates_root, candidate)? {
                written += 1;
            }
        }
    }
    Ok(written)
}

fn candidate_for(cluster: &[(&LessonRecord, f64)]) -> Option<Candidate> {
    let anchor = cluster[0].0;
    let projects: BTreeSet<&str> = cluster.iter().map(|(r, _)| r.project_id.as_str()).collect();
    let has_tag = |wanted: &[&str]| {
        cluster.iter().any(|(r, _)| {
            r.tags
                .iter()
                .any(|tag| wanted.iter().any(|w| tag.eq_ignore_ascii_case(w)))
        })
    };
    let tagged_global = has_tag(GLOBAL_TAGS);
    let tagged_user = has_tag(USER_TAGS);

    let mut reasons = Vec::new();
    if projects.len() >= 2 {
        reasons.push("recurrence".to_owned());
    }
    if tagged_global {
        reasons.push("tagged-global".to_owned());
    }
    if names_dependency_or_cli(&anchor.text)
        && !cluster.iter().any(|(r, _)| has_repo_relative_path(&r.text))
    {
        reasons.push("portable-tooling".to_owned());
    }
    if reasons.is_empty() {
        return None;
    }

    let scope_tags: Vec<&str> = GLOBAL_TAGS.iter().chain(USER_TAGS).copied().collect();
    let tags: BTreeSet<String> = cluster
        .iter()
        .flat_map(|(r, _)| r.tags.iter())
        .filter(|tag| !scope_tags.iter().any(|s| tag.eq_ignore_ascii_case(s)))
        .cloned()
        .collect();
    let now = Utc::now().to_rfc3339();
    Some(Candidate {
        schema_version: 1,
        id: format!("promo-{}", &hex_digest(&anchor.lesson_id)[..16]),
        kind: "promotion".to_owned(),
        state: "pending".to_owned(),
        scope: if tagged_user && !tagged_global {
            "user"
        } else {
            "global"
        }
        .to_owned(),
        reasons,
        title: title_for(&anchor.text),
        content: anchor.text.clone(),
        tags: tags.into_iter().collect(),
        fingerprint: anchor.lesson_id.clone(),
        evidence: cluster
            .iter()
            .map(|(r, similarity)| Evidence {
                lesson_id: r.lesson_id.clone(),
                project_id: r.project_id.clone(),
                project_root: r.project_root.clone(),
                source: r.source.clone(),
                session_id: r.session_id.clone(),
                recorded_at: r.recorded_at.clone(),
                similarity: (similarity * 1000.0).round() / 1000.0,
            })
            .collect(),
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Write a candidate unless it was already decided or is unchanged. Returns
/// whether a file was written.
fn write_candidate(root: &Path, mut candidate: Candidate) -> Result<bool> {
    let filename = format!("{}.json", candidate.id);
    if root.join("accepted").join(&filename).exists()
        || root.join("rejected").join(&filename).exists()
    {
        return Ok(false);
    }
    let pending = root.join("pending").join(&filename);
    if pending.exists() {
        let existing: Value = serde_json::from_slice(&fs::read(&pending)?)?;
        let existing_ids: BTreeSet<&str> = existing["evidence"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["lessonId"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let new_ids: BTreeSet<&str> = candidate
            .evidence
            .iter()
            .map(|e| e.lesson_id.as_str())
            .collect();
        if existing_ids == new_ids {
            return Ok(false);
        }
        if let Some(created_at) = existing["createdAt"].as_str() {
            candidate.created_at = created_at.to_owned();
        }
    }
    atomic_json(&pending, &candidate)?;
    Ok(true)
}

fn extract_lessons(message: &str) -> Vec<String> {
    let mut lessons = Vec::new();
    for line in message.lines() {
        let line = line.trim().trim_start_matches(['-', '*', ' ']).trim();
        for prefix in LESSON_PREFIXES {
            let matches = line
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix));
            if matches {
                let body = line[prefix.len()..].trim();
                if !body.is_empty() {
                    lessons.push(body.chars().take(LESSON_TEXT_LIMIT).collect());
                }
                break;
            }
        }
    }
    if lessons.is_empty() {
        let whole = message.trim();
        if !whole.is_empty() {
            lessons.push(whole.chars().take(LESSON_TEXT_LIMIT).collect());
        }
    }
    lessons
}

/// Inline `#tag` words, e.g. `#global` or `#scope:user`.
fn hashtags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = text
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('#'))
        .map(|tag| tag.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == ':')))
        .filter(|tag| {
            !tag.is_empty()
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '-')
        })
        .map(str::to_ascii_lowercase)
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn shingles(tokens: &[String]) -> Vec<u64> {
    let mut hashes: Vec<u64> = if tokens.len() < SHINGLE_SIZE {
        vec![shingle_hash(&tokens.join(" "))]
    } else {
        tokens
            .windows(SHINGLE_SIZE)
            .map(|window| shingle_hash(&window.join(" ")))
            .collect()
    };
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

fn names_dependency_or_cli(text: &str) -> bool {
    let in_code_span = text.split('`').skip(1).step_by(2).any(|span| {
        let span = span.trim();
        let first = span.split_whitespace().next().unwrap_or("");
        KNOWN_CLIS.contains(&first.to_ascii_lowercase().as_str())
            || is_versioned_name(first)
            || is_manifest_pin(span)
    });
    in_code_span
        || text
            .split_whitespace()
            .map(|word| word.trim_matches(|c: char| "`'\"(),;".contains(c)))
            .any(is_versioned_name)
}

/// `name@1.2.3`, including scoped `@scope/name@1.2`.
fn is_versioned_name(word: &str) -> bool {
    let Some((name, version)) = word.rsplit_once('@') else {
        return false;
    };
    let name = name.trim_start_matches('@');
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        && version
            .trim_start_matches(['^', '~', '=', 'v'])
            .starts_with(|c: char| c.is_ascii_digit())
}

/// A manifest pin such as `tokio = "1.40"` or `flutter_rust_bridge = "=2.12.0"`.
fn is_manifest_pin(span: &str) -> bool {
    let Some((name, value)) = span.split_once('=') else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        && value
            .trim()
            .trim_start_matches('"')
            .trim_start_matches(['^', '~', '=', '<', '>'])
            .starts_with(|c: char| c.is_ascii_digit())
}

fn has_repo_relative_path(text: &str) -> bool {
    text.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| "`'\"()[]{},;".contains(c));
        let word = word.split(':').next().unwrap_or("").trim_end_matches('.');
        if !word.contains('/') || word.contains("://") || word.starts_with(['/', '~', '$', '@']) {
            return false;
        }
        if word.starts_with("./") || word.starts_with("../") {
            return true;
        }
        let segments: Vec<&str> = word.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() < 2 {
            return false;
        }
        let last = segments[segments.len() - 1];
        REPO_DIRS.contains(&segments[0])
            || last
                .rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
    })
}

fn title_for(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(80).collect();
    if first_line.chars().count() > 80 {
        title.push('…');
    }
    if title.is_empty() {
        "Promoted lesson".to_owned()
    } else {
        title
    }
}

fn hex_digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
