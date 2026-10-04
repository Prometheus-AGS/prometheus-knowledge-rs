## Why

The skill system's team-aware learning work needs recall to target a single agent role.
Three gaps prevented that:

- **`pk context` never saw most of a large knowledge base.** It took `ceil(max_candidates / scopes)` entries per scope in snapshot order *before* scoring them. With 200 entries and default flags, a late-sorting match was never returned.
- **The learning worker wrote unattributed records and nothing recallable.** It always sent `agent_id: null`, and it never committed the prompt snapshot that `pk context` reads.
- **pk could not classify or filter entries by role, team or visibility.**

This change records three surface changes that already merged: #31, #32 and #34. This repository requires an OpenSpec change naming the consumers for any change to a frozen surface, and those PRs were merged without one. This proposal is that record.

## What Changes

- **`pk context --format json` (frozen surface).**
  - Every snapshot entry of every readable scope is scored. `--max-candidates` now caps the merged, ranked list.
  - Ranking is deterministic: score descending, then scope priority, then id.
  - New key `scored_count`.
  - **Changed meaning:** `candidate_count` was "entries inspected (capped)". It is now "ranked candidates kept after the cap".
  - `--format hook` output is unchanged.
  - The contract test `pk-cli/tests/context.rs::candidate_budget_is_shared_across_requested_scopes` asserted the old inspected count. It now asserts `scored_count = 5` and `candidate_count = 1`. That is a semantic update, not a weakening: the budget is still enforced, now on the ranked list.
- **`pk context --tag <t>` (new flag, additive).** Repeatable, all-of, applied before scoring.
- **`pk ingest --type <T>` and `--tag <t>` (new flags, additive).**
  - `--type` sets the OKF entry type. `Reference` remains the default.
  - `--tag` adds tags, with caller tags first and duplicates removed.
  - The rust API gains `Librarian::compile_with(raw, CompileOverrides)`. `compile()` delegates to it.
- **Learning job schema.**
  - Optional `projectId`, `teamId` and `roleId`, all `serde(default)`. The job stays at schemaVersion 2, and older jobs still parse.
  - The memory scope keys are never null: `user_id` = project, `agent_id` = `<team>/<role>`, else `@project`. Shared scope uses `@global`.
  - The worker commits the prompt snapshot after each upsert.
  - Hash-bound queued payloads are not rewritten. Only the legacy `add_task_step` form gains keys.

## Consumers

| Consumer | Affected surface | Effect |
|---|---|---|
| prometheus-skill-system (hooks, recall, memory bridge) | `pk context --format json`, `pk ingest`, learning job fields | Intended consumer: it reads `results` and now passes `--tag`/`--type` and the job identity fields. It reads no `candidate_count`. |
| prometheus-skills-mini (`lib/karpathy/transport.mjs`) | `pk ingest` | Additive flags only. Existing invocations are unchanged. |
| forge-rs | `pk ingest` (shells out) | Additive flags only. Existing invocations are unchanged. |
| prometheus-cli | pk-core / pk-store / pk-librarian crates | `compile_with` is additive. No type changed. |
| prometheus doctor | `status.json` | Unchanged. |

## Capabilities

### New Capabilities
- `team-aware-recall`: context scoring and budget, tag filtering, ingest classification, and attributed learning jobs.

## Impact

- **Code:** `pk-cli/src/main.rs`, `pk-librarian/src/librarian.rs` + `lib.rs`, `pk-learning-worker/src/main.rs`.
- **Tests:** `pk-cli/tests/context_scoring.rs`, `pk-cli/tests/tags_and_type.rs`, `pk-learning-worker/tests/attribution.rs`, and the updated assertion in `pk-cli/tests/context.rs`.
- **Release:** the workspace version goes from 1.9.0 to **1.10.0**, a minor bump. All the changes are additive except the `candidate_count` meaning, which no consumer reads.
