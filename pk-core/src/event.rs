use crate::types::{ArticleId, LintReport};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Events emitted on the Librarian's tokio::sync::broadcast bus.
/// Consumed by the MCP SSE stream, Cherry plugin, and UAR AG-UI firehose.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LibrarianEvent {
    /// A RawDoc was successfully compiled into a WikiEntry
    Compiled {
        entry_id: ArticleId,
        title: String,
        tags: Vec<String>,
        ts: DateTime<Utc>,
    },

    /// A lint pass completed
    LintCompleted {
        reports: Vec<LintReport>,
        total_entries_scanned: usize,
        ts: DateTime<Utc>,
    },

    /// A focused mini-KB was produced for a topic
    Focused {
        topic: String,
        entry_count: usize,
        ts: DateTime<Utc>,
    },

    /// An existing entry was updated (by lint auto-fix or manual patch)
    Updated {
        entry_id: ArticleId,
        revision: u32,
        ts: DateTime<Utc>,
    },

    /// A new RawDoc arrived in the inbox (watcher event)
    RawDocArrived {
        source_path: String,
        ts: DateTime<Utc>,
    },

    /// General error that did not crash the librarian
    Error { message: String, ts: DateTime<Utc> },
}

impl LibrarianEvent {
    pub fn compiled(entry_id: ArticleId, title: String, tags: Vec<String>) -> Self {
        Self::Compiled {
            entry_id,
            title,
            tags,
            ts: chrono::Utc::now(),
        }
    }

    pub fn lint_completed(reports: Vec<LintReport>, scanned: usize) -> Self {
        Self::LintCompleted {
            reports,
            total_entries_scanned: scanned,
            ts: chrono::Utc::now(),
        }
    }

    pub fn error(msg: impl std::fmt::Display) -> Self {
        Self::Error {
            message: msg.to_string(),
            ts: chrono::Utc::now(),
        }
    }
}
