## ADDED Requirements

### Requirement: The worker fingerprints lessons
After processing jobs, `prometheus-learning-worker run-once` SHALL append a record for each newly seen lesson to `~/.prometheus/learning-index/lessons.jsonl`, carrying its project id, project root, source evidence and word-trigram fingerprint. Re-processing the same lesson SHALL NOT append a second record.

#### Scenario: Re-running indexes nothing new
- **WHEN** the worker runs again with no new jobs
- **THEN** `lessons.jsonl` is byte-identical

### Requirement: Recurring or portable lessons become promotion candidates
The worker SHALL write a candidate to `~/.prometheus/promotion-candidates/pending/<id>.json`, atomically, with an evidence list, when a lesson recurs with Jaccard ≥ 0.6 across at least two distinct projects, is tagged global without a `[GLOBAL]`/`[USER]` marker, or names a dependency or CLI and contains no repo-relative path. A lesson carrying an immediate-promotion marker SHALL NOT be proposed.

#### Scenario: The same lesson in two projects
- **WHEN** two jobs from two different projects carry the same lesson
- **THEN** exactly one pending candidate exists, with reason `recurrence` and two evidence entries citing two distinct project ids

### Requirement: Candidate proposal is idempotent
Re-running the worker SHALL NOT create a second candidate for the same cluster, SHALL NOT rewrite a pending candidate whose evidence is unchanged, and SHALL NOT re-propose a candidate that was accepted or rejected.

#### Scenario: Re-run adds none
- **WHEN** the worker runs a second time with no new jobs
- **THEN** the pending directory holds the same single file, byte-identical

### Requirement: Accepting a promotion candidate promotes it
`pk candidates accept <id>` SHALL upsert the lesson into `~/.prometheus/knowledge/shared`, commit the shared prompt snapshot, queue one schemaVersion 2 `add_memory` operation whose `payloadHash` is the SHA-256 of its serialized arguments, keyed `@global`/`@global` (or `@user:<hash>`/`@user` for a user-scoped candidate), and move the candidate file to `accepted/`. Accepting an already-accepted candidate SHALL fail without queuing a second operation.

#### Scenario: Accept a global candidate
- **WHEN** `pk candidates accept <id>` runs against a pending global candidate
- **THEN** the shared prompt snapshot contains `promoted-<id>`, the learning-queue outbox holds one `@global` `add_memory` operation, and the candidate is in `accepted/`

### Requirement: Rejecting a candidate retires it
`pk candidates reject <id>` SHALL move the candidate to `rejected/`, recording an optional reason.

#### Scenario: Reject
- **WHEN** `pk candidates reject <id> --reason noise` runs
- **THEN** the file is in `rejected/` and no longer in `pending/`

### Requirement: Skill candidates are listed but not yet accepted
`pk candidates --kind skill` SHALL list and reject entries under `~/.prometheus/skill-candidates/`, and `accept` SHALL exit non-zero with a "not yet supported" message, leaving the file in place.

#### Scenario: Skill accept
- **WHEN** `pk candidates accept <id> --kind skill` runs
- **THEN** it exits non-zero, stderr says "not yet supported", and the candidate is still pending
