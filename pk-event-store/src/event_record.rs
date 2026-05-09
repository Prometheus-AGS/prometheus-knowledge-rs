//! `EventRecord` — the persistent form of a `LibrarianEvent`.

use chrono::{DateTime, Utc};
use pk_core::LibrarianEvent;
use serde::{Deserialize, Serialize};

/// The on-disk / database form of a LibrarianEvent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// Stable unique ID (UUID v4)
    pub id: String,
    /// Discriminant: "compiled", "lint_completed", "focused", "updated",
    /// "raw_doc_arrived", "error"
    pub kind: String,
    /// Session identifier (set from SESSION_ID env var or generated once per process)
    pub session_id: String,
    /// Absolute project root when the event was emitted
    pub project_root: String,
    /// Per-project scope (matches SP-008 KbScope: "project" or "shared")
    pub scope: String,
    /// ISO-8601 timestamp
    pub timestamp: DateTime<Utc>,
    /// Event-specific payload (verbatim serialization of the LibrarianEvent variant)
    pub payload: serde_json::Value,
    /// IDs of WikiEntries this event directly affected (empty for non-entry events)
    pub affects: Vec<String>,
}

impl EventRecord {
    pub fn from_event(
        event: &LibrarianEvent,
        session_id: &str,
        project_root: &str,
        scope: &str,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let (kind, affects, ts) = describe(event);
        let payload = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);

        Self {
            id,
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            project_root: project_root.to_string(),
            scope: scope.to_string(),
            timestamp: ts,
            payload,
            affects,
        }
    }
}

fn describe(event: &LibrarianEvent) -> (&'static str, Vec<String>, DateTime<Utc>) {
    match event {
        LibrarianEvent::Compiled { entry_id, ts, .. } => {
            ("compiled", vec![entry_id.to_string()], *ts)
        }
        LibrarianEvent::LintCompleted { ts, .. } => ("lint_completed", vec![], *ts),
        LibrarianEvent::Focused { ts, .. } => ("focused", vec![], *ts),
        LibrarianEvent::Updated { entry_id, ts, .. } => {
            ("updated", vec![entry_id.to_string()], *ts)
        }
        LibrarianEvent::RawDocArrived { ts, .. } => ("raw_doc_arrived", vec![], *ts),
        LibrarianEvent::Error { ts, .. } => ("error", vec![], *ts),
    }
}
