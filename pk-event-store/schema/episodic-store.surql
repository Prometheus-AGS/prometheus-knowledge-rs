-- SP-020: Episodic store schema
-- Namespace: prometheus  Database: episode
-- Apply with: surreal import --conn http://localhost:8000 --ns prometheus --db episode episodic-store.surql

-- ── Sessions ──────────────────────────────────────────────────────────────────

DEFINE TABLE episode_session SCHEMAFULL;

DEFINE FIELD id           ON episode_session TYPE string;
DEFINE FIELD project_root ON episode_session TYPE string;
DEFINE FIELD started_at   ON episode_session TYPE datetime DEFAULT time::now();
DEFINE FIELD ended_at     ON episode_session TYPE option<datetime>;
DEFINE FIELD summary      ON episode_session TYPE option<string>;
DEFINE FIELD tags         ON episode_session TYPE array DEFAULT [];
-- Configurable retention: default 90 days, overridable per project
DEFINE FIELD expires_at   ON episode_session TYPE datetime DEFAULT time::now() + 90d;

DEFINE INDEX episode_session_project ON episode_session FIELDS project_root;
DEFINE INDEX episode_session_time    ON episode_session FIELDS started_at;
DEFINE INDEX episode_session_expiry  ON episode_session FIELDS expires_at;

-- ── Interactions (individual events within a session) ─────────────────────────

DEFINE TABLE episode_interaction SCHEMAFULL;

DEFINE FIELD id           ON episode_interaction TYPE string;
DEFINE FIELD session_id   ON episode_interaction TYPE string;
DEFINE FIELD project_root ON episode_interaction TYPE string;
DEFINE FIELD kind         ON episode_interaction TYPE string;  -- matches LibrarianEvent.kind
DEFINE FIELD timestamp    ON episode_interaction TYPE datetime DEFAULT time::now();
DEFINE FIELD payload      ON episode_interaction TYPE object;
DEFINE FIELD affects      ON episode_interaction TYPE array;   -- array<string> of KG entity IDs
-- Inherits expiry from parent session; set explicitly for direct-write interactions
DEFINE FIELD expires_at   ON episode_interaction TYPE datetime DEFAULT time::now() + 90d;

DEFINE INDEX episode_interaction_session ON episode_interaction FIELDS session_id;
DEFINE INDEX episode_interaction_project ON episode_interaction FIELDS project_root;
DEFINE INDEX episode_interaction_kind    ON episode_interaction FIELDS kind;
DEFINE INDEX episode_interaction_time    ON episode_interaction FIELDS timestamp;

-- ── Artifacts (outputs produced during a session) ─────────────────────────────

DEFINE TABLE episode_artifact SCHEMAFULL;

DEFINE FIELD id           ON episode_artifact TYPE string;
DEFINE FIELD session_id   ON episode_artifact TYPE string;
DEFINE FIELD artifact_type ON episode_artifact TYPE string;  -- 'wiki_entry', 'trace', 'video', 'report'
DEFINE FIELD artifact_id  ON episode_artifact TYPE string;   -- ID in the artifact's native store
DEFINE FIELD created_at   ON episode_artifact TYPE datetime DEFAULT time::now();
DEFINE FIELD expires_at   ON episode_artifact TYPE datetime DEFAULT time::now() + 90d;

DEFINE INDEX episode_artifact_session ON episode_artifact FIELDS session_id;
DEFINE INDEX episode_artifact_type    ON episode_artifact FIELDS artifact_type;

-- ── Bridge: episode interaction mentions KG entity ────────────────────────────

DEFINE TABLE episode_mentions SCHEMAFULL;
DEFINE FIELD interaction_id ON episode_mentions TYPE string;
DEFINE FIELD kg_entity_id   ON episode_mentions TYPE string;
DEFINE FIELD mention_type   ON episode_mentions TYPE string DEFAULT 'referenced';

DEFINE INDEX episode_mentions_interaction ON episode_mentions FIELDS interaction_id;
DEFINE INDEX episode_mentions_entity      ON episode_mentions FIELDS kg_entity_id;
