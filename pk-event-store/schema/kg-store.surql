-- SP-020: Knowledge Graph store schema
-- Namespace: prometheus  Database: kg
-- Apply with: surreal import --conn http://localhost:8000 --ns prometheus --db kg kg-store.surql

-- ── Entities ──────────────────────────────────────────────────────────────────

DEFINE TABLE kg_entity SCHEMAFULL;

DEFINE FIELD id          ON kg_entity TYPE string;
DEFINE FIELD name        ON kg_entity TYPE string;
DEFINE FIELD entity_type ON kg_entity TYPE string;   -- 'concept', 'skill', 'crate', 'person', 'project', etc.
DEFINE FIELD observations ON kg_entity TYPE array;   -- array<string>: observations about this entity
DEFINE FIELD project_root ON kg_entity TYPE option<string>;  -- null = global
DEFINE FIELD created_at  ON kg_entity TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at  ON kg_entity TYPE datetime DEFAULT time::now();

DEFINE INDEX kg_entity_name ON kg_entity FIELDS name;
DEFINE INDEX kg_entity_type ON kg_entity FIELDS entity_type;
DEFINE INDEX kg_entity_project ON kg_entity FIELDS project_root;

-- ── Relations ─────────────────────────────────────────────────────────────────

DEFINE TABLE kg_relation SCHEMAFULL TYPE RELATION FROM kg_entity TO kg_entity;
DEFINE FIELD in            ON kg_relation TYPE record<kg_entity>;
DEFINE FIELD out           ON kg_relation TYPE record<kg_entity>;
DEFINE FIELD relation_type ON kg_relation TYPE string;  -- 'depends_on', 'related_to', 'part_of', etc.
DEFINE FIELD weight        ON kg_relation TYPE float DEFAULT 1.0;
DEFINE FIELD created_at    ON kg_relation TYPE datetime DEFAULT time::now();

DEFINE INDEX kg_relation_type ON kg_relation FIELDS relation_type;

-- ── Properties ────────────────────────────────────────────────────────────────

DEFINE TABLE kg_property SCHEMAFULL;
DEFINE FIELD entity_id ON kg_property TYPE string;
DEFINE FIELD key       ON kg_property TYPE string;
DEFINE FIELD value     ON kg_property TYPE any;
DEFINE FIELD source    ON kg_property TYPE option<string>;  -- where this property came from

DEFINE INDEX kg_property_entity ON kg_property FIELDS entity_id;
DEFINE INDEX kg_property_key    ON kg_property FIELDS key;

-- ── Bridge: KG entity derived from episodic session ───────────────────────────

DEFINE TABLE kg_derived_from SCHEMAFULL;
DEFINE FIELD kg_entity_id   ON kg_derived_from TYPE string;
DEFINE FIELD episode_session_id ON kg_derived_from TYPE string;
DEFINE FIELD confidence     ON kg_derived_from TYPE float DEFAULT 1.0;

DEFINE INDEX kg_derived_entity ON kg_derived_from FIELDS kg_entity_id;
