## Purpose

Ensures that ingesting content that is the same as, or a near-duplicate of, previously-ingested content updates the existing wiki entry instead of creating a new one, so the wiki never accumulates redundant `Reference` entries for the same underlying fact.

## ADDED Requirements

### Requirement: Duplicate content updates the existing entry
When raw content submitted for compilation is a duplicate or near-duplicate of content already represented by an existing wiki entry, the system SHALL update that existing entry (incrementing its revision) instead of creating a new entry, regardless of any difference in the title synthesized for the incoming content.

#### Scenario: Identical content ingested twice with different synthesized titles
- **WHEN** the same raw content is ingested twice, and the compiler synthesizes two differently-worded titles for the two calls
- **THEN** the wiki contains exactly one entry for that content, and its revision number after the second ingest is one greater than after the first

#### Scenario: Near-duplicate content ingested with only cosmetic wording differences
- **WHEN** raw content is ingested that differs from an existing entry's source content only in incidental wording (not in the facts recorded)
- **THEN** the existing entry is updated in place rather than a new entry being created

### Requirement: Genuinely distinct content still creates a new entry
The system SHALL continue to create a new wiki entry when incoming content is not a duplicate or near-duplicate of any existing entry, so the deduplication behavior never suppresses legitimately new knowledge.

#### Scenario: Unrelated content ingested after a duplicate-detection pass
- **WHEN** raw content is ingested whose subject matter and facts do not match any existing entry
- **THEN** a new wiki entry is created for it, distinct from all existing entries

### Requirement: Deduplication does not depend on a vector database
The system SHALL determine content duplication using only TF-IDF-based similarity and/or deterministic content hashing already available in the store, consistent with the project's no-vector-database constraint.

#### Scenario: Duplicate detection runs without any external embedding or vector service
- **WHEN** the system evaluates whether incoming content duplicates an existing entry
- **THEN** the evaluation completes using only in-process TF-IDF search and/or content hashing, with no call to an external vector database or embedding service
