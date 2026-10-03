use crate::{
    index::TextIndex,
    markdown::{article_path, entry_to_markdown, is_reserved_filename, markdown_to_entry},
};
use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, LintReport, RawDoc, WikiEntry},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub struct MarkdownStore {
    wiki_dir: PathBuf,
    raw_dir: PathBuf,
    inner: Arc<RwLock<StoreInner>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreReconcileReport {
    pub indexed_count: usize,
    pub on_disk_count: usize,
    pub parse_failures: usize,
    pub changed: bool,
    pub last_reload: chrono::DateTime<chrono::Utc>,
}

struct StoreInner {
    entries: HashMap<ArticleId, WikiEntry>,
    index: TextIndex,
    source_hashes: HashMap<PathBuf, String>,
    content_hash_index: HashMap<String, ArticleId>,
    report: StoreReconcileReport,
}

struct ScanOutcome {
    entries: HashMap<ArticleId, WikiEntry>,
    index: TextIndex,
    source_hashes: HashMap<PathBuf, String>,
    content_hash_index: HashMap<String, ArticleId>,
    on_disk_count: usize,
    parse_failures: usize,
}

/// Read the ingest content-hash stamped on an entry's `extra` frontmatter
/// (see `crate::dedup::CONTENT_HASH_KEY`), if present.
fn entry_content_hash(entry: &WikiEntry) -> Option<String> {
    entry
        .extra
        .get(crate::dedup::CONTENT_HASH_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

impl MarkdownStore {
    /// Open (or create) the store at `base_path`.
    /// Expects:
    ///   base_path/wiki/   — compiled markdown articles
    ///   base_path/raw/    — incoming unprocessed docs
    pub async fn open(base_path: impl AsRef<Path>) -> PkResult<Self> {
        let base = base_path.as_ref();
        let wiki_dir = base.join("wiki");
        let raw_dir = base.join("raw");

        tokio::fs::create_dir_all(&wiki_dir).await?;
        tokio::fs::create_dir_all(&raw_dir).await?;

        let scan = scan_wiki_tree(&wiki_dir).await?;
        let now = chrono::Utc::now();
        let inner = StoreInner {
            report: StoreReconcileReport {
                indexed_count: scan.entries.len(),
                on_disk_count: scan.on_disk_count,
                parse_failures: scan.parse_failures,
                changed: true,
                last_reload: now,
            },
            entries: scan.entries,
            index: scan.index,
            source_hashes: scan.source_hashes,
            content_hash_index: scan.content_hash_index,
        };

        info!(
            count = inner.entries.len(),
            parse_failures = inner.report.parse_failures,
            wiki_dir = %wiki_dir.display(),
            "store loaded"
        );

        Ok(Self {
            wiki_dir,
            raw_dir,
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    /// Reconcile the in-memory index with the complete on-disk wiki tree.
    /// Canonical paths and SHA-256 content hashes make direct writes, renames,
    /// and deletions deterministic. A fresh index is swapped under one write
    /// lock, so readers never observe a partially reloaded store.
    pub async fn reconcile_from_disk(&self) -> PkResult<StoreReconcileReport> {
        let scan = scan_wiki_tree(&self.wiki_dir).await?;
        let mut inner = self.inner.write().await;
        if scan.parse_failures > 0 {
            inner.report = StoreReconcileReport {
                indexed_count: inner.entries.len(),
                on_disk_count: scan.on_disk_count,
                parse_failures: scan.parse_failures,
                changed: false,
                last_reload: inner.report.last_reload,
            };
            return Ok(inner.report.clone());
        }
        let changed = inner.source_hashes != scan.source_hashes;
        let last_reload = if changed {
            chrono::Utc::now()
        } else {
            inner.report.last_reload
        };
        if changed {
            inner.entries = scan.entries;
            inner.index = scan.index;
            inner.source_hashes = scan.source_hashes;
            inner.content_hash_index = scan.content_hash_index;
        }
        inner.report = StoreReconcileReport {
            indexed_count: inner.entries.len(),
            on_disk_count: scan.on_disk_count,
            parse_failures: scan.parse_failures,
            changed,
            last_reload,
        };
        Ok(inner.report.clone())
    }

    pub async fn readiness_report(&self) -> StoreReconcileReport {
        self.inner.read().await.report.clone()
    }

    pub async fn upsert(&self, mut entry: WikiEntry) -> PkResult<WikiEntry> {
        if !entry.id.is_safe_path() {
            return Err(PkError::frontmatter(format!(
                "id {:?} is not a safe concept path (no leading '/', no '\\' or ':', and no segment that is empty, '..', a Windows device name, or ends in '.' or a space)",
                entry.id.as_str()
            )));
        }

        let file_content = {
            let mut inner = self.inner.write().await;

            if inner.entries.contains_key(&entry.id) {
                entry.bump_revision();
            }

            if let Some(previous) = inner.entries.get(&entry.id) {
                if let Some(stale_hash) = entry_content_hash(previous) {
                    if entry_content_hash(&entry).as_deref() != Some(stale_hash.as_str()) {
                        inner.content_hash_index.remove(&stale_hash);
                    }
                }
            }
            if let Some(hash) = entry_content_hash(&entry) {
                inner.content_hash_index.insert(hash, entry.id.clone());
            }

            inner.index.upsert(&entry);
            let id = entry.id.clone();
            inner.entries.insert(id, entry.clone());
            entry_to_markdown(&entry)?
        };

        let path = article_path(&self.wiki_dir, &entry.id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, file_content).await?;
        debug!(id = %entry.id, path = %path.display(), "entry flushed");

        Ok(entry)
    }

    pub async fn delete(&self, id: &ArticleId) -> PkResult<()> {
        {
            let mut inner = self.inner.write().await;
            let Some(removed) = inner.entries.remove(id) else {
                return Err(PkError::not_found(id));
            };
            if let Some(hash) = entry_content_hash(&removed) {
                inner.content_hash_index.remove(&hash);
            }
            inner.index.remove(id);
        }

        let path = article_path(&self.wiki_dir, id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    pub async fn get(&self, id: &ArticleId) -> PkResult<WikiEntry> {
        self.inner
            .read()
            .await
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| PkError::not_found(id))
    }

    pub async fn snapshot(&self) -> PkResult<Vec<WikiEntry>> {
        let inner = self.inner.read().await;
        Ok(inner.entries.values().cloned().collect())
    }

    /// Regenerate the wiki-root `index.md` (OKF v0.2 §8) from the pages on disk.
    /// Called after every ingest so the catalog stays current.
    ///
    /// Holds an exclusive lock on `wiki/.index.lock` across scan, merge, render
    /// and write, so concurrent rebuilds (other stores, other processes) are
    /// serialized. Renders from a fresh disk scan rather than this store's
    /// in-memory snapshot, keeps existing index lines for pages this binary
    /// cannot parse, and replaces the file atomically (issue #15).
    pub async fn regenerate_index(&self) -> PkResult<()> {
        let lock_path = self.wiki_dir.join(INDEX_LOCK_FILENAME);
        let lock = tokio::task::spawn_blocking(move || acquire_exclusive_lock(&lock_path))
            .await
            .map_err(join_error)??;

        let result = self.rebuild_index_locked().await;
        // Closing the descriptor releases the lock; unlock explicitly anyway.
        let _ = fs2::FileExt::unlock(&lock);
        drop(lock);
        result
    }

    async fn rebuild_index_locked(&self) -> PkResult<()> {
        let scan = scan_wiki_tree(&self.wiki_dir).await?;
        let entries: Vec<WikiEntry> = scan.entries.values().cloned().collect();
        let parsed_ids: HashSet<ArticleId> = scan.entries.into_keys().collect();
        let rendered = crate::bundle::render_index(&entries);

        let path = self.wiki_dir.join("index.md");
        let existing = match tokio::fs::read(&path).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let content =
            crate::bundle::merge_preserved_index_entries(&rendered, &existing, &parsed_ids, |id| {
                article_path(&self.wiki_dir, &ArticleId::from(id)).is_file()
            });
        write_atomic(&path, content).await?;
        debug!(
            path = %path.display(),
            parse_failures = scan.parse_failures,
            "index.md regenerated"
        );
        Ok(())
    }

    /// Append an entry to the wiki-root `log.md` (OKF v0.2 §9) under today's date
    /// group, newest first. `action` is the leading bold verb (`Creation`,
    /// `Update`, …).
    pub async fn append_log(&self, action: &str, title: &str, id: &ArticleId) -> PkResult<()> {
        let path = self.wiki_dir.join("log.md");
        let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let line = format!("* **{action}**: [{title}](/{}.md)", id.as_str());
        let updated = crate::bundle::append_log_line(&existing, &date, &line);
        write_atomic(&path, updated).await?;
        debug!(path = %path.display(), action, "log.md appended");
        Ok(())
    }

    pub async fn entry_count(&self) -> usize {
        self.inner.read().await.entries.len()
    }

    /// Scan the wiki tree and return OKF v0.2 §11 conformance reports
    /// (deterministic; no LLM). Reads raw files so it can flag documents the
    /// store skipped on load (e.g. unparseable frontmatter), and checks the
    /// reserved `index.md`/`log.md` structure. Orphan detection uses the
    /// in-memory snapshot.
    pub async fn okf_conformance_reports(&self) -> PkResult<Vec<LintReport>> {
        let mut concept_files: Vec<(String, String)> = Vec::new();
        let mut index_raw: Option<String> = None;
        let mut log_raw: Option<String> = None;

        let mut dirs = vec![self.wiki_dir.clone()];
        while let Some(dir_path) = dirs.pop() {
            let mut rd = tokio::fs::read_dir(&dir_path).await?;
            while let Some(entry) = rd.next_entry().await? {
                let path = entry.path();
                if entry.file_type().await?.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                let at_root = dir_path == self.wiki_dir;
                if crate::markdown::is_reserved_filename(name) {
                    // Only the bundle-root index.md/log.md get structure checks.
                    if at_root && name == "index.md" {
                        index_raw = Some(content);
                    } else if at_root && name == "log.md" {
                        log_raw = Some(content);
                    }
                    continue;
                }
                let relative = path.strip_prefix(&self.wiki_dir).unwrap_or(&path);
                let id: String = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let id = id.trim_end_matches(".md").to_string();
                concept_files.push((id, content));
            }
        }

        let known_ids: HashSet<String> = concept_files.iter().map(|(id, _)| id.clone()).collect();

        let mut reports = Vec::new();
        for (id, raw) in &concept_files {
            reports.extend(crate::bundle::okf_document_reports(id, raw, &known_ids));
        }
        let snapshot = self.snapshot().await?;
        reports.extend(crate::bundle::okf_orphan_reports(&snapshot));
        if let Some(idx) = index_raw {
            reports.extend(crate::bundle::okf_index_reports(&idx));
        }
        if let Some(log) = log_raw {
            reports.extend(crate::bundle::okf_log_reports(&log));
        }
        Ok(reports)
    }

    /// Deterministically fix an entry missing a non-empty OKF `type` by
    /// assigning the generic default and re-persisting it. Returns the fixed
    /// entry, or `None` if the entry already had a type (nothing to fix).
    pub async fn okf_autofix_type(&self, id: &ArticleId) -> PkResult<Option<WikiEntry>> {
        let mut entry = self.get(id).await?;
        if entry
            .entry_type
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            entry.entry_type = Some("Reference".to_string());
            return Ok(Some(self.upsert(entry).await?));
        }
        Ok(None)
    }

    /// Look up an existing entry by its stamped ingest content hash (see
    /// `crate::dedup::normalized_content_hash`). Used to detect that
    /// incoming raw content is a byte-for-byte (post-normalization) repeat
    /// of content already compiled, independent of any title the compiler
    /// synthesizes for it this time around.
    pub async fn find_by_content_hash(&self, hash: &str) -> Option<ArticleId> {
        self.inner
            .read()
            .await
            .content_hash_index
            .get(hash)
            .cloned()
    }

    pub async fn related_entries(&self, doc: &RawDoc, k: usize) -> PkResult<Vec<WikiEntry>> {
        let query: &str = &doc.content[..doc.content.len().min(500)];
        self.search(query, k).await
    }

    pub async fn search(&self, query: &str, k: usize) -> PkResult<Vec<WikiEntry>> {
        Ok(self
            .search_scored(query, k)
            .await?
            .into_iter()
            .map(|(entry, _score)| entry)
            .collect())
    }

    /// Search the transparent TF-IDF index while retaining scores for callers
    /// that need to merge results from more than one knowledge scope.
    pub async fn search_scored(&self, query: &str, k: usize) -> PkResult<Vec<(WikiEntry, f32)>> {
        let inner = self.inner.read().await;
        let ranked = inner.index.search(query, k);
        let results = ranked
            .into_iter()
            .filter_map(|(id, score)| inner.entries.get(&id).cloned().map(|entry| (entry, score)))
            .collect();
        Ok(results)
    }

    pub fn raw_dir(&self) -> &Path {
        &self.raw_dir
    }

    pub fn wiki_dir(&self) -> &Path {
        &self.wiki_dir
    }

    pub async fn write_raw(&self, filename: &str, content: &str) -> PkResult<PathBuf> {
        let path = self.raw_dir.join(filename);
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }
}

/// Lock file serializing `index.md` rebuilds. No `.md` extension, so the
/// wiki scan never treats it as a concept page.
const INDEX_LOCK_FILENAME: &str = ".index.lock";

fn join_error(error: tokio::task::JoinError) -> PkError {
    PkError::from(std::io::Error::other(error))
}

/// Open (creating if needed) `lock_path` and take a blocking exclusive `fs2`
/// lock on it. Call only from a blocking context. The lock lives as long as
/// the returned `File`.
fn acquire_exclusive_lock(lock_path: &Path) -> std::io::Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    fs2::FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

/// Replace `path` atomically: write a uniquely named temp file in the same
/// directory, sync it, then rename it over the target. A crash leaves either
/// the old file or the new one, never a truncated one.
async fn write_atomic(path: &Path, contents: String) -> PkResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_atomic_blocking(&path, contents.as_bytes()))
        .await
        .map_err(join_error)??;
    Ok(())
}

fn write_atomic_blocking(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let written = write_synced(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn write_synced(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

async fn scan_wiki_tree(wiki_dir: &Path) -> PkResult<ScanOutcome> {
    let mut outcome = ScanOutcome {
        entries: HashMap::new(),
        index: TextIndex::new(),
        source_hashes: HashMap::new(),
        content_hash_index: HashMap::new(),
        on_disk_count: 0,
        parse_failures: 0,
    };
    // Canonicalizing every article schedules one blocking filesystem task per
    // file. Large project stores made the learning worker spend minutes in
    // allocator and filesystem pressure before it could process one event.
    // Resolve the stable root once and preserve each relative article path.
    let canonical_wiki_dir = tokio::fs::canonicalize(wiki_dir)
        .await
        .unwrap_or_else(|_| wiki_dir.to_path_buf());
    let mut dirs_to_visit = vec![wiki_dir.to_path_buf()];

    while let Some(dir_path) = dirs_to_visit.pop() {
        let mut dir = tokio::fs::read_dir(&dir_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                dirs_to_visit.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if is_reserved_filename(file_name) {
                debug!(path = %path.display(), "skipping reserved OKF filename");
                continue;
            }

            outcome.on_disk_count += 1;
            let relative = path.strip_prefix(wiki_dir).unwrap_or(&path);
            let canonical_path = canonical_wiki_dir.join(relative);
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(content) => content,
                Err(error) => {
                    outcome.parse_failures += 1;
                    warn!(path = %path.display(), err = %error, "failed to read entry");
                    continue;
                }
            };
            outcome.source_hashes.insert(
                canonical_path,
                format!("{:x}", Sha256::digest(content.as_bytes())),
            );

            let fallback_id: String = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let fallback_id = fallback_id.trim_end_matches(".md");
            match markdown_to_entry(&content, Some(fallback_id)) {
                Ok(wiki_entry) => {
                    debug!(id = %wiki_entry.id, "loaded entry");
                    outcome.index.upsert(&wiki_entry);
                    if let Some(hash) = entry_content_hash(&wiki_entry) {
                        outcome
                            .content_hash_index
                            .insert(hash, wiki_entry.id.clone());
                    }
                    outcome.entries.insert(wiki_entry.id.clone(), wiki_entry);
                }
                Err(error) => {
                    outcome.parse_failures += 1;
                    warn!(path = %path.display(), err = %error, "skipping malformed entry");
                }
            }
        }
    }
    Ok(outcome)
}
