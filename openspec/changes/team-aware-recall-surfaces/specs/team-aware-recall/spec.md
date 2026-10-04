## ADDED Requirements

### Requirement: Context scores every entry before applying the candidate cap
`pk context` SHALL score every entry of every readable scope, and SHALL apply `--max-candidates` only to the merged, de-duplicated, ranked list. A scope that fails SHALL NOT reserve any share of the cap.

#### Scenario: A late-sorting entry in a large scope is recalled
- **WHEN** a project scope holds 200 committed entries and the only match sorts last by id
- **THEN** `pk context <token> --format json`, with default flags, returns that entry, and `scored_count` is 200

#### Scenario: A failed scope does not reserve budget
- **WHEN** the project scope fails and the shared scope holds 60 matching entries, with `--max-candidates 60`
- **THEN** `candidate_count` is 60

### Requirement: Context output is deterministic
Results SHALL be ordered by score descending, then scope priority (project, shared, global), then entry id, and repeated runs over one snapshot SHALL produce byte-identical JSON.

#### Scenario: Two runs are identical
- **WHEN** the same `pk context` command runs twice against unchanged snapshots
- **THEN** stdout is byte-identical

### Requirement: Context filters by tags
`pk context --tag <t>`, repeated, SHALL keep only entries that carry every given tag. The filter SHALL run before scoring.

#### Scenario: All-of tag filter
- **WHEN** entries carry `role:api-dev`, and `role:ui-dev` together with `vis:team`
- **THEN** `--tag role:api-dev` returns only the first, `--tag role:ui-dev --tag vis:team` returns only the second, and `--tag role:api-dev --tag vis:team` returns nothing

### Requirement: Ingest classifies entries
`pk ingest --type <T>` SHALL set the entry type, with `Reference` as the default when it is absent. `--tag <t>`, repeated, SHALL add tags ahead of the model's tags, without duplicates.

#### Scenario: Typed and tagged ingest
- **WHEN** a document is ingested with `--type Lesson --tag role:api-dev`
- **THEN** its frontmatter has `type: Lesson`, and its tags include `role:api-dev` and the model's tags

### Requirement: Learning jobs produce attributed memory scope keys
The worker SHALL derive `user_id` from the job's `projectId`, falling back to the project scope. It SHALL derive `agent_id` as `<teamId>/<roleId>` when both are present, otherwise `@project`. Shared scope SHALL use `@global` for both. No queued memory operation the worker creates SHALL have a null `user_id` or `agent_id`.

#### Scenario: Attributed job
- **WHEN** a job carries projectId `project:x`, teamId `t` and roleId `r`
- **THEN** its memory operation has `user_id = project:x` and `agent_id = t/r`

### Requirement: The worker publishes recallable snapshots
After a successful upsert, the worker SHALL commit the prompt snapshot for the target knowledge base.

#### Scenario: A session record is recallable
- **WHEN** the worker processes a job
- **THEN** the committed project prompt snapshot contains the job's session entry
