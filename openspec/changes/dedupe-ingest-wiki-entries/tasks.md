## 1. Content-hash foundation

- [x] 1.1 Add a normalized-content-hash helper (SHA-256 hex over whitespace-normalized raw content) with unit tests: identical content (byte-for-byte and with only whitespace differences) hashes equal; distinct content hashes different. Implemented as `pk_store::dedup::normalized_content_hash` (pk-store already depends on `sha2`; pk-core does not) — see `pk-store/src/dedup.rs`.
- [x] 1.2 Confirm `WikiEntry`'s existing `extra: BTreeMap<String, serde_yaml::Value>` round-trips a `content_hash` key through parse → serialize, and that OKF conformance checks don't flag it. `pk_store::dedup::stamp_content_hash` + `stamp_content_hash_round_trips_through_extra` test; `extra` already round-trips per existing `unknown_frontmatter_keys_round_trip` coverage in `pk-store/src/markdown.rs`, and `okf_document_reports` never inspects `extra` keys, so no conformance impact.

## 2. Duplicate lookup (pk-store)

- [x] 2.1 Add a `content_hash` index to `MarkdownStore` (`content_hash_index: HashMap<String, ArticleId>` on `StoreInner`, maintained in `scan_wiki_tree`/`upsert`/`delete`) and `find_by_content_hash()`. Covered by `duplicate_content_updates_existing_entry_instead_of_creating_new` in `pk-store/tests/store_tests.rs`.
- [x] 2.2 Add a near-duplicate helper. Implemented as `pk_store::dedup::find_near_duplicate` using Jaccard word-overlap over normalized text rather than raw `search_scored()` scores — `TextIndex::search`'s tf·idf sum is unbounded (depends on corpus size), so a fixed threshold against it would be unreliable; word-overlap ratio is a bounded `[0,1]` measure independent of corpus size. Unit tests cover a true positive (near-duplicate wording) and true negative (distinct content).

## 3. Compile-path integration (pk-librarian)

- [x] 3.1 `Librarian::compile()` computes the content hash from `raw.content` and checks `store.find_by_content_hash()` before finalizing the entry.
- [x] 3.2 When no exact hash match exists, checks `find_near_duplicate()` against the existing `related_entries()` result set already fetched for prompt context (no extra TF-IDF pass).
- [x] 3.3 Reuses the matched entry's `ArticleId` and stamps `extra["content_hash"]` before `upsert()`. Also fixed `is_new`/OKF-log labeling to read the *returned* (post-upsert) entry's revision rather than the pre-upsert value (which `WikiEntry::new` always sets to 0) — the original code computed `is_new` before `upsert()` ran, so it was always `true` regardless of whether the id collided; this was a latent bug this fix's id-reuse path would otherwise have made visible as mislabeled "Creation" log entries on every merge.
- [x] 3.4 Verified: `distinct_content_still_creates_a_new_entry` / `distinct_content_creates_a_new_entry` tests confirm unrelated content still gets a fresh id and `revision == 0`.

## 4. Regression coverage

- [x] 4.1 No HTTP-mocking infra exists for `LlmClient` in this crate (concrete struct, not a trait; no `wiremock`-style dev-dependency), so a true `compile()`-level test isn't feasible without adding new test infrastructure out of this fix's scope. Instead added `duplicate_content_updates_existing_entry_instead_of_creating_new` in `pk-store/tests/store_tests.rs`, which exercises the exact production sequence `compile()` runs (hash → lookup → near-dup fallback → id reuse → upsert) via a `compile_like_ingest` helper, asserting one entry / `revision == 1` after two ingests of identical content with two different synthesized titles.
- [x] 4.2 `distinct_content_still_creates_a_new_entry` covers the false-positive-suppression case.
- [x] 4.3 `cargo test --workspace` — all green, 0 failed, including `upsert_is_idempotent` and `compiled_entries_default_to_reference_type` unmodified.

## 5. Close out

- [x] 5.1 `cargo fmt --all -- --check` clean (after one `cargo fmt --all` pass) and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] 5.2 Close GitHub issue #7 on `Prometheus-AGS/prometheus-knowledge-rs`, referencing the merged fix commit — pending commit/push.
