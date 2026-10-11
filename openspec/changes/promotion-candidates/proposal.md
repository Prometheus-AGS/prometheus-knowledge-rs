## Why

The skill system's team-aware learning design (§3, "Visibility and promotion") makes promotion beyond a project auto-proposed and human-confirmed. Nothing proposed or applied promotion candidates:

- **No detector.** A lesson learned in two projects stayed two private project lessons. Nothing noticed recurrence, a `global` tag, or a lesson about a tool rather than a repository.
- **No confirmation step.** No command took a proposed lesson into the shared KB and the `@global`/`@user` memory scopes, so the only way to promote was a hand-run `pk ingest --scope shared`, which skips surreal-memory entirely.

Adding a `pk` subcommand changes a frozen surface (the `pk` CLI). This proposal names the consumers.

## What Changes

- **Worker promotion detector (new, additive).** After draining jobs in `run-once`, `prometheus-learning-worker`:
  - extracts lessons from each processed job (lines prefixed `LESSON:`, `GOTCHA:`, `PATTERN:` or `LEARNED:`, else the whole final assistant message) and from typed `Lesson`/`Gotcha` entries in the project KB;
  - fingerprints them as word-trigram shingle sets into `~/.prometheus/learning-index/lessons.jsonl`, with project id, project root and source evidence. Append-only and idempotent on lesson id;
  - proposes a candidate when a lesson recurs with Jaccard ≥ 0.6 across ≥ 2 distinct projects, is tagged global without a `[GLOBAL]`/`[USER]` marker, or names a dependency or CLI and contains no repo-relative path;
  - writes candidates atomically to `~/.prometheus/promotion-candidates/pending/<id>.json` with an evidence list. Re-runs do not duplicate a candidate, and a candidate already accepted or rejected is never re-proposed.
- **`pk candidates list|accept|reject --kind promotion|skill` (new subcommand, additive).**
  - `list [--state pending|accepted|rejected] [--json]`.
  - `accept <id>` (promotion): upserts the lesson into `~/.prometheus/knowledge/shared` as `promoted-<id>`, commits the shared prompt snapshot, queues an `add_memory` operation for `@global` (or `@user:<hash>` / `@user` for a user-scoped candidate) in the learning-queue memory outbox, and moves the file to `accepted/`.
  - `reject <id> [--reason]`: moves the file to `rejected/`.
  - `--kind skill` reads `~/.prometheus/skill-candidates/`. `list` and `reject` work. `accept` exits non-zero with "not yet supported" until skill promotion lands.

## Consumers

| Consumer | Affected surface | Effect |
|---|---|---|
| prometheus-skill-system (`kbd-open.sh`, reflector role, later skill-candidate work) | `pk candidates`, `~/.prometheus/promotion-candidates/` | Intended consumer. It surfaces pending candidates and asks a human to run `pk candidates accept|reject`. Needs a pin bump to this release; handoff note below. |
| prometheus-skills-mini | `pk ingest` | Unchanged. |
| forge-rs | `pk ingest` (shells out) | Unchanged. |
| prometheus-cli | pk-core / pk-store / pk-librarian crates | No public API change. |
| prometheus doctor | `status.json` | No field added. `lastError` may now carry a `promotion detector: …` message. |
| surreal-memory-server | `/api/v2/operations` via the worker's outbox | New operations use the existing `add_memory` method and the existing schemaVersion 2 outbox format, keyed `@global`/`@global` or `@user:<hash>`/`@user`. No ledger contract change. |

## Capabilities

### New Capabilities
- `promotion-candidates`: lesson fingerprinting, candidate proposal, and human-confirmed promotion to shared scopes.

## Impact

- **Code:** `pk-learning-worker/src/promotion.rs` (new), `pk-learning-worker/src/main.rs`, `pk-cli/src/candidates.rs` (new), `pk-cli/src/main.rs`.
- **Tests:** `pk-learning-worker/tests/promotion.rs`, `pk-cli/tests/candidates.rs`.
- **Dependencies:** none added. `Cargo.lock` is unchanged.
- **Downstream handoff (prometheus-skill-system):** bump the pk pin to the release containing this change. Document `pk candidates` in `docs/guide/13-tools-reference.md`, and point `kbd-open.sh` and the reflector role at `~/.prometheus/promotion-candidates/pending`.
