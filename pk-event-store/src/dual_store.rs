//! SP-020: Dual-store routing for LibrarianEvent persistence.
//!
//! Routes events to the appropriate store:
//! - KG store (`kg` database): knowledge-derivation events (compile, lint_completed)
//! - Episodic store (`episode` database): session-scoped events (all others)
//!
//! When SURREAL_URL is not set, both routes fall back to the same JSONL file.

use crate::EventRecord;
use anyhow::Result;

/// Which logical store a LibrarianEvent should be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreTarget {
    /// Knowledge graph database — durable, factual, long-lived
    KnowledgeGraph,
    /// Episodic database — temporal, session-scoped, expires after 90 days
    Episodic,
}

impl StoreTarget {
    pub fn surreal_db(&self) -> &'static str {
        match self {
            StoreTarget::KnowledgeGraph => "kg",
            StoreTarget::Episodic => "episode",
        }
    }

    pub fn surreal_table(&self) -> &'static str {
        match self {
            StoreTarget::KnowledgeGraph => "kg_entity",
            StoreTarget::Episodic => "episode_interaction",
        }
    }
}

/// Classify an event record into a store target based on its kind.
pub fn classify(record: &EventRecord) -> StoreTarget {
    match record.kind.as_str() {
        // KG-producing events: the output is a durable knowledge artifact
        "compiled" | "lint_completed" | "updated" => StoreTarget::KnowledgeGraph,
        // Episodic events: session-scoped, temporal
        _ => StoreTarget::Episodic,
    }
}

/// Write a record to the appropriate SurrealDB table via HTTP.
pub async fn write_to_target(
    client: &reqwest::Client,
    base_url: &str,
    surreal_ns: &str,
    record: &EventRecord,
) -> Result<()> {
    let target = classify(record);
    let url = format!("{}/key/{}/{}", base_url, target.surreal_table(), record.id);

    let resp = client
        .post(&url)
        .header("NS", surreal_ns)
        .header("DB", target.surreal_db())
        .header("Accept", "application/json")
        .json(record)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "SurrealDB POST {url} (db={}) returned {status}: {body}",
            target.surreal_db()
        );
    }
    Ok(())
}
