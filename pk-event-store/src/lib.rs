//! First-class LibrarianEvent persistence for surreal-memory (SP-019).
//!
//! Events are written to a SurrealDB instance (via HTTP API) keyed by
//! SURREAL_URL / SURREAL_NS / SURREAL_DB env vars. When those vars are absent,
//! the store operates in "offline" mode and appends events to a JSONL fallback
//! at `<project_root>/.prometheus/events.jsonl`.

pub mod event_record;
pub mod fallback;
pub mod store;

pub use event_record::EventRecord;
pub use store::EventStore;
