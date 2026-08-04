//! `EventStore` — writes `EventRecord` to SurrealDB (via HTTP API) or falls
//! back to a local JSONL file when SURREAL_URL is not set.

use crate::{fallback::JsonlFallback, EventRecord};
use anyhow::Result;
use pk_core::LibrarianEvent;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

pub struct EventStore {
    project_root: PathBuf,
    session_id: String,
    scope: String,
    surreal_url: Option<String>,
    surreal_ns: String,
    #[allow(dead_code)]
    surreal_db: String,
    client: reqwest::Client,
    fallback: JsonlFallback,
}

impl EventStore {
    /// Create an EventStore for `project_root`.
    ///
    /// Connection parameters are read from env vars:
    ///   SURREAL_URL  — e.g. http://localhost:8000
    ///   SURREAL_NS   — default "prometheus"
    ///   SURREAL_DB   — default "librarian"
    ///   SESSION_ID   — human-readable session identifier (optional)
    pub fn for_project(project_root: &Path, scope: &str) -> Self {
        let session_id =
            std::env::var("SESSION_ID").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
        let surreal_url = std::env::var("SURREAL_URL").ok();
        let surreal_ns = std::env::var("SURREAL_NS").unwrap_or_else(|_| "prometheus".into());
        let surreal_db = std::env::var("SURREAL_DB").unwrap_or_else(|_| "librarian".into());

        Self {
            project_root: project_root.to_owned(),
            session_id,
            scope: scope.to_string(),
            surreal_url,
            surreal_ns,
            surreal_db,
            client: reqwest::Client::new(),
            fallback: JsonlFallback::new(project_root),
        }
    }

    /// Persist a `LibrarianEvent`. Writes to SurrealDB if available; falls back
    /// to JSONL append on failure or when SURREAL_URL is not configured.
    pub async fn persist(&self, event: &LibrarianEvent) -> Result<()> {
        let record = EventRecord::from_event(
            event,
            &self.session_id,
            &self.project_root.to_string_lossy(),
            &self.scope,
        );

        if let Some(ref base_url) = self.surreal_url {
            match self.write_to_surreal(base_url, &record).await {
                Ok(()) => {
                    debug!(event_id = %record.id, kind = %record.kind, "event persisted to surreal-memory");
                    return Ok(());
                }
                Err(e) => {
                    warn!("surreal-memory write failed (falling back to JSONL): {e}");
                }
            }
        }

        self.fallback.append(&record)?;
        debug!(event_id = %record.id, kind = %record.kind, "event appended to JSONL fallback");
        Ok(())
    }

    /// Write a record to the appropriate SurrealDB store (dual-store routing — SP-020).
    async fn write_to_surreal(&self, base_url: &str, record: &EventRecord) -> Result<()> {
        crate::dual_store::write_to_target(&self.client, base_url, &self.surreal_ns, record).await
    }

    /// List recent events for this project.
    pub fn list(&self, kind: Option<&str>, limit: usize) -> Result<Vec<EventRecord>> {
        self.fallback
            .list(&self.project_root.to_string_lossy(), kind, limit)
    }

    /// Return derivation history for a specific entry ID.
    pub fn for_entry(&self, entry_id: &str) -> Result<Vec<EventRecord>> {
        self.fallback
            .for_entry(entry_id, &self.project_root.to_string_lossy())
    }
}
