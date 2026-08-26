## Context

See proposal.md - Why for the motivating bug (GitHub issue #7). Relevant current state:

- `Librarian::compile()` (`pk-librarian/src/librarian.rs:39`) always constructs a brand-new `WikiEntry` from the LLM's JSON response via `parse_compile_response()` (`librarian.rs:265`), which calls `WikiEntry::new(title, content)`.
- `WikiEntry::new` (`pk-core/src/types.rs`) sets `id = ArticleId::from_slug(&title)` and `revision = 0` unconditionally — identity and "is this new" are both functions of the LLM-synthesized title only.
- `MarkdownStore::upsert()` (`pk-store/src/store.rs:133`) treats an entry as an update only when `inner.entries.contains_key(&entry.id)` — an exact `ArticleId` match.
- `WikiEntry` already carries an `extra: BTreeMap<String, serde_yaml::Value>` field that preserves unknown frontmatter keys verbatim across parse/serialize round-trips (OKF §9 permissive consumption) — this is a safe place to add a producer-extension field without touching the OKF-required schema.
- `related_entries()` (`pk-store/src/store.rs:308`) already runs a TF-IDF search over the incoming raw content and returns up to 5 candidate existing entries — currently used only as prompt context.
- Project constraint: no vector databases; stay TF-IDF/hash-based.

## Goals / Non-Goals

**Goals:**
- Detect when compiled content duplicates (exactly or near-exactly) an existing wiki entry's underlying content, independent of title wording.
- On detection, merge into the existing entry (bump revision) instead of creating a new `ArticleId`.
- Keep genuinely new content unaffected — no increase in false-positive merges.
- Stay within the existing TF-IDF/hash toolchain; no new runtime dependency.

**Non-Goals:**
- General-purpose semantic dedup across unrelated topics (only near-duplicate detection of the *same* ingested fact, as in the reported bug).
- Retroactively merging the 153 already-duplicated entries described in issue #7 — this change fixes the ingest path going forward; a one-off cleanup of existing duplicates is a separate, optional follow-up not included here.
- Changing the OKF-required frontmatter schema (`id`, `title`, `tags`, `links`, `sources`, `created_at`, `updated_at`, `revision`, `type`, `description` stay as-is).

## Decisions

### 1. Exact-duplicate detection via a normalized content hash stored in `extra`
Compute a hash (e.g. SHA-256, truncated/hex-encoded) over the raw ingested content, normalized (trim whitespace, collapse internal whitespace, lowercase where safe) so trivial formatting differences don't defeat matching. Store it as `extra["content_hash"]` on the `WikiEntry` at compile time.

Before `compile()` finalizes a new entry, look up whether any entry already in the store carries the same `content_hash` (a new `MarkdownStore` lookup method, e.g. `find_by_content_hash`, backed by a `HashMap<String, ArticleId>` built alongside the existing TF-IDF index). If found, merge into that entry (see Decision 3) instead of treating the compiled result as new.

**Why a hash over a similarity score for the exact case**: deterministic, O(1) lookup, zero false positives for the reported bug's actual failure mode (byte-identical status lines re-ingested repeatedly). A hash-based check needs no model call and runs before the LLM compile step even completes, so it's cheap to check first.

**Alternatives considered**: hashing the *compiled* `WikiEntry.content` instead of the raw input — rejected, because the LLM's synthesized markdown body can itself vary slightly between calls even over identical input (temperature 0.2, per `client.complete(..., 0.2)`), which would reintroduce the same problem one level down. Hashing the raw input (before the LLM ever sees it) is the correct anchor for identity.

### 2. Near-duplicate detection via existing TF-IDF `related_entries()` results
For content that isn't a byte-identical repeat (e.g. the issue's "cosmetic wording differences" case) but scores very highly against an existing entry in `related_entries()`, treat a match above a fixed similarity threshold as a duplicate candidate too, subject to the same merge path.

**Why reuse `related_entries()` rather than add a second search**: it already runs on every `compile()` call and already returns scored candidates (`search_scored`) internally; wiring the threshold check through the same call avoids a second TF-IDF pass. The threshold starts as a fixed constant (not user-configurable) chosen conservatively high to avoid false-positive merges — exact tuning is a task-level detail, not a design fork.

**Alternatives considered**: embedding-based cosine similarity — rejected outright, violates the project's no-vector-database constraint.

### 3. Merge behavior: reuse existing `id`, `upsert()` semantics do the rest
When a duplicate/near-duplicate is detected, construct the entry to write with the *existing* matched entry's `ArticleId` (not a fresh slug from the new title) before calling `store.upsert()`. `upsert()` already bumps revision automatically when `inner.entries.contains_key(&entry.id)` is true (`pk-store/src/store.rs:143`), so no change is needed to `upsert()` itself — only to what `compile()` passes into it. `compile()`'s existing `is_new` flag (`entry.revision == 0`) then naturally reports "Update" in the OKF log, since the reused entry won't have `revision == 0` (unless it's the first sighting of that content).

**Why not add a separate `merge()` method**: `upsert()`'s existing update path already does the right thing (bump revision, overwrite content, re-index) once the id is correct. Fixing the id-selection step ahead of `upsert()` is the minimal correct change and avoids two divergent write paths.

## Risks / Trade-offs

- **[Risk]** A too-aggressive similarity threshold merges genuinely distinct content into an unrelated entry, silently losing information → **Mitigation**: start with exact-hash matching as the primary path (zero false positives) and keep the TF-IDF threshold conservative; cover both directions (true positive on the reported bug's exact-repeat case, true negative on distinct content) in regression tests before tuning further.
- **[Risk]** Merging can overwrite content a human hand-edited between ingests → **Mitigation**: out of scope for this change (no evidence of human-edited wiki files in the reported bug); `upsert()`'s existing revision bump preserves history via git, so an unwanted merge is recoverable.
- **[Risk]** Existing duplicate files from before this fix are not cleaned up → **Mitigation**: explicitly a non-goal (see above); tracked as a possible follow-up, not blocking this fix.

## Migration Plan

No data migration required — `content_hash` is an additive `extra` key written going forward; existing entries simply lack it until next re-ingested/updated. No rollback concerns beyond reverting the code change (file format is unaffected for entries that are never re-touched).
