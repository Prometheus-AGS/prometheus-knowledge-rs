use crate::{
    index::TextIndex,
    markdown::{article_path, entry_to_markdown, is_reserved_filename, markdown_to_entry},
};
use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, LintReport, RawDoc, WikiEntry},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub struct MarkdownStore {
    wiki_dir: PathBuf,
    raw_dir: PathBuf,
    inner: Arc<RwLock<StoreInner>>,
}

struct StoreInner {
    entries: HashMap<ArticleId, WikiEntry>,
    index: TextIndex,
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

        let mut inner = StoreInner {
            entries: HashMap::new(),
            index: TextIndex::new(),
        };

        // OKF v0.1 §3: a bundle is a directory TREE — subdirectories group
        // concepts (§3.1's reserved filenames apply "at any level of the
        // hierarchy"). Walk recursively so nested concepts load like root
        // ones.
        let mut dirs_to_visit = vec![wiki_dir.clone()];
        while let Some(dir_path) = dirs_to_visit.pop() {
            let mut dir = tokio::fs::read_dir(&dir_path).await?;
            while let Some(entry) = dir.next_entry().await? {
                let path = entry.path();
                let file_type = entry.file_type().await?;

                if file_type.is_dir() {
                    dirs_to_visit.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                // OKF v0.1 §3.1: index.md and log.md are reserved bundle
                // files at any level — skip them rather than trying (and
                // failing) to parse them as wiki entries.
                if is_reserved_filename(file_name) {
                    debug!(path = %path.display(), "skipping reserved OKF filename");
                    continue;
                }

                // Concept ID (OKF §2) = wiki-relative path minus `.md`,
                // forward-slash-joined regardless of host path separator.
                let relative = path.strip_prefix(&wiki_dir).unwrap_or(&path);
                let fallback_id: String = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let fallback_id = fallback_id.trim_end_matches(".md");

                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => match markdown_to_entry(&content, Some(fallback_id)) {
                        Ok(wiki_entry) => {
                            debug!(id = %wiki_entry.id, "loaded entry");
                            inner.index.upsert(&wiki_entry);
                            inner.entries.insert(wiki_entry.id.clone(), wiki_entry);
                        }
                        Err(e) => warn!(path = %path.display(), err = %e, "skipping malformed entry"),
                    },
                    Err(e) => warn!(path = %path.display(), err = %e, "failed to read entry"),
                }
            }
        }

        info!(
            count = inner.entries.len(),
            wiki_dir = %wiki_dir.display(),
            "store loaded"
        );

        Ok(Self {
            wiki_dir,
            raw_dir,
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    pub async fn upsert(&self, mut entry: WikiEntry) -> PkResult<WikiEntry> {
        if !entry.id.is_safe_path() {
            return Err(PkError::frontmatter(format!(
                "id {:?} is not a safe concept path (no leading '/', no '..' or empty segments)",
                entry.id.as_str()
            )));
        }

        let file_content = {
            let mut inner = self.inner.write().await;

            if inner.entries.contains_key(&entry.id) {
                entry.bump_revision();
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
            if inner.entries.remove(id).is_none() {
                return Err(PkError::not_found(id));
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

    /// Regenerate the wiki-root `index.md` (OKF §6) from the current entries.
    /// Called after every ingest so the catalog stays current.
    pub async fn regenerate_index(&self) -> PkResult<()> {
        let entries = self.snapshot().await?;
        let content = crate::bundle::render_index(&entries);
        let path = self.wiki_dir.join("index.md");
        tokio::fs::write(&path, content).await?;
        debug!(path = %path.display(), "index.md regenerated");
        Ok(())
    }

    /// Append an entry to the wiki-root `log.md` (OKF §7) under today's date
    /// group, newest first. `action` is the leading bold verb (`Creation`,
    /// `Update`, …).
    pub async fn append_log(&self, action: &str, title: &str, id: &ArticleId) -> PkResult<()> {
        let path = self.wiki_dir.join("log.md");
        let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let line = format!("* **{action}**: [{title}](/{}.md)", id.as_str());
        let updated = crate::bundle::append_log_line(&existing, &date, &line);
        tokio::fs::write(&path, updated).await?;
        debug!(path = %path.display(), action, "log.md appended");
        Ok(())
    }

    pub async fn entry_count(&self) -> usize {
        self.inner.read().await.entries.len()
    }

    /// Scan the wiki tree and return OKF v0.1 §9 conformance reports
    /// (deterministic; no LLM). Reads raw files so it can flag documents the
    /// store skipped on load (e.g. unparseable frontmatter), and checks the
    /// reserved `index.md`/`log.md` structure. Orphan detection uses the
    /// in-memory snapshot.
    pub async fn okf_conformance_reports(&self) -> PkResult<Vec<LintReport>> {
        use std::collections::HashSet;

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

        let known_ids: HashSet<String> =
            concept_files.iter().map(|(id, _)| id.clone()).collect();

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

    pub async fn related_entries(&self, doc: &RawDoc, k: usize) -> PkResult<Vec<WikiEntry>> {
        let query: &str = &doc.content[..doc.content.len().min(500)];
        self.search(query, k).await
    }

    pub async fn search(&self, query: &str, k: usize) -> PkResult<Vec<WikiEntry>> {
        let inner = self.inner.read().await;
        let ranked = inner.index.search(query, k);
        let results = ranked
            .into_iter()
            .filter_map(|(id, _score)| inner.entries.get(&id).cloned())
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
