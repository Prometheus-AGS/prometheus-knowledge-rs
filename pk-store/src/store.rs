use crate::{
    index::TextIndex,
    markdown::{article_path, entry_to_markdown, is_reserved_filename, markdown_to_entry},
};
use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, RawDoc, WikiEntry},
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

        let mut dir = tokio::fs::read_dir(&wiki_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // OKF v0.1 §3.1: index.md and log.md are reserved bundle files,
            // never concept documents — skip them rather than trying (and
            // failing) to parse them as wiki entries.
            if is_reserved_filename(file_name) {
                debug!(path = %path.display(), "skipping reserved OKF filename");
                continue;
            }
            let fallback_id = file_name.trim_end_matches(".md");
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

    pub async fn entry_count(&self) -> usize {
        self.inner.read().await.entries.len()
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
