-- SP-019: LibrarianEvent persistence schema for SurrealDB
-- Apply with: surreal import --conn http://localhost:8000 --ns prometheus --db librarian events.surql

-- ── Event table ───────────────────────────────────────────────────────────────

DEFINE TABLE event SCHEMAFULL;

DEFINE FIELD id           ON event TYPE string;
DEFINE FIELD kind         ON event TYPE string;   -- 'compiled' | 'lint_completed' | 'focused' | 'updated' | 'raw_doc_arrived' | 'error'
DEFINE FIELD session_id   ON event TYPE string;
DEFINE FIELD project_root ON event TYPE string;
DEFINE FIELD scope        ON event TYPE string;   -- 'project' | 'shared'
DEFINE FIELD timestamp    ON event TYPE datetime DEFAULT time::now();
DEFINE FIELD payload      ON event TYPE object;   -- event-kind-specific data
DEFINE FIELD affects      ON event TYPE array;    -- array<string> of WikiEntry IDs

-- Indexes for common query patterns
DEFINE INDEX event_kind          ON event FIELDS kind;
DEFINE INDEX event_project_ts    ON event FIELDS project_root, timestamp;
DEFINE INDEX event_session        ON event FIELDS session_id;

-- ── Relation: event affects wiki_entry ────────────────────────────────────────

DEFINE TABLE affects SCHEMAFULL TYPE RELATION FROM event TO wiki_entry;
DEFINE FIELD in  ON affects TYPE record<event>;
DEFINE FIELD out ON affects TYPE record<wiki_entry>;
DEFINE INDEX affects_in  ON affects FIELDS in;
DEFINE INDEX affects_out ON affects FIELDS out;

-- ── Relation: event compiled_from (derivation chain) ─────────────────────────

DEFINE TABLE compiled_from SCHEMAFULL TYPE RELATION FROM event TO event;
DEFINE FIELD in  ON compiled_from TYPE record<event>;
DEFINE FIELD out ON compiled_from TYPE record<event>;
