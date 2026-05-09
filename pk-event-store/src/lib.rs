//! First-class LibrarianEvent persistence for surreal-memory (SP-019 + SP-020).
//!
//! SP-019: events are written to SurrealDB (via HTTP API) or fall back to a
//! JSONL file at `<project_root>/.prometheus/events.jsonl`.
//!
//! SP-020: dual-store routing classifies events into the knowledge-graph
//! store (`kg` database) or the episodic store (`episode` database).
//! A migration command reshards the legacy single-store JSONL file.

pub mod dual_store;
pub mod event_record;
pub mod fallback;
pub mod migrate;
pub mod store;

pub use event_record::EventRecord;
pub use store::EventStore;
