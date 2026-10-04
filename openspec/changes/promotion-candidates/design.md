## Where the detector runs

Inside `run-once`, after jobs drain and before memory reconciliation, while the queue lock is held. The worker is the only writer of `lessons.jsonl` and `pending/`, so the lock serializes it. A detector error is logged and recorded in `status.json`'s existing `lastError`. It never fails the run, because memory delivery matters more than a proposal.

## What a "lesson" is

The worker's job carries one final assistant message, not structured lessons. The detector takes lines prefixed `LESSON:`, `GOTCHA:`, `PATTERN:` or `LEARNED:` (case-insensitive, list bullets allowed). When there are none, it takes the whole message (first 2,000 characters). It also takes typed `Lesson`/`Gotcha` entries from the project KB, because frontmatter tags are the only place a `global` tag is observable. Inline `#tag` words count as tags for session lessons.

**The uncomfortable case:** a whole-message lesson is noisy. Two long, mostly different messages rarely reach Jaccard 0.6, which keeps false recurrence low. But a message that names a CLI in a code span and cites no repo path becomes a `portable-tooling` candidate on its own. The human gate absorbs that noise. If it proves too much, the fix is to tighten `names_dependency_or_cli`, not to remove the gate.

## Fingerprints and ids

- **Tokens:** lowercase alphanumeric runs. **Shingles:** word trigrams, hashed with the first 8 bytes of SHA-256. A text under three tokens is one shingle. SHA-256, not `DefaultHasher`, because the index outlives the binary and `DefaultHasher` output is not stable across Rust releases.
- **Lesson id:** SHA-256 over `projectId \0 source \0 normalized tokens`, so re-indexing the same job or KB entry is a no-op.
- **Clustering:** greedy, single pass, ordered by `(recordedAt, lessonId)`. Each record joins the first cluster whose anchor (first member) it matches at ≥ 0.6. New records always sort after existing ones, so an anchor never changes.
- **Candidate id:** `promo-` plus 16 hex digits of SHA-256 over the anchor's lesson id. It stays stable as evidence grows.

## Idempotency

A candidate whose id exists in `accepted/` or `rejected/` is skipped. A pending candidate is rewritten, atomically and keeping `createdAt`, only when its evidence set changed. Otherwise the file is untouched.

**Known race:** `pk candidates accept` does not take the worker's queue lock, because pk-cli has no `fs2` dependency. If a worker run rewrites a pending candidate at the moment `accept` renames it, the pending file can reappear. A second accept is then idempotent: the KB upsert is skipped when the content is unchanged, and the memory operation is skipped when its id exists in any outbox state. The window is the time between one `exists` check and one `rename`.

## Accept semantics

The steps run in a crash-safe order:

1. Upsert the KB entry. It has a fixed id and is skipped when unchanged.
2. Commit the shared snapshot.
3. Queue the memory operation. Its id is deterministic, `sha256("promotion-candidate:<id>")`, and it is skipped when present in any `memory/*` state.
4. Rewrite the candidate with `state: accepted`, then durably rename it to `accepted/`.

A crash at any step leaves a pending candidate, and re-running `accept` completes it without duplicates.

The outbox record matches the worker's own `enqueue_memory`: schemaVersion 2, `method: add_memory`, and `payloadHash` = SHA-256 of `serde_json::to_vec(arguments)`. The worker's `normalize_operation` re-derives that hash, and it passes `add_memory` arguments through unchanged.

**User scope** is `@user:<hash>` with `agent_id: @user` (design table §2). The hash uses the same resolver as the skill system's `project_id.py`: `PROMETHEUS_USER_ID`, else the first 16 hex digits of SHA-256 over the lowercased global git email.

## Timestamps in pk-cli

pk-cli has no chrono dependency, and adding one would change `Cargo.lock`, which the `--locked` rule forbids without approval. `rfc3339_now()` formats `SystemTime` with the civil-from-days algorithm. The worker parses `queuedAt` as RFC 3339, so the format must stay `YYYY-MM-DDTHH:MM:SSZ`.
