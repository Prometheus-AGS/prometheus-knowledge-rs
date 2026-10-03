# Verifier findings: issue #15 preserve-wiki-index

- Range: `main (46b08eb)..fix/preserve-wiki-index (602c856)`, commits 937cf4c, 5e71337, 5151070, 9e1e962, 40442b6, 602c856
- Date: 2026-10-03
- Review type: **separate-context review (same model family); human PR review required before merge**
- Verdict: **PASS WITH FINDINGS** (0 BLOCKER, 0 MAJOR, 5 MINOR, 6 NIT)

## Summary

The issue #15 failure modes are fixed. A stale in-memory snapshot no longer drops entries: the index is now rendered from a fresh disk scan under an exclusive lock. Index lines for pages that exist on disk but fail to parse are carried over verbatim. The `okf_version` block is always emitted. `index.md` and `log.md` are replaced atomically, and `append_log` shares the lock. Locking is sound: the blocking `lock_exclusive` runs in `spawn_blocking`, the lock is held across scan, merge, render and write, the two methods never nest, and cancellation drops the `File`, which releases the lock. If every upsert is followed by its own rebuild, the last rebuild sees all pages. The public API change is additive: `bundle::merge_preserved_index_entries` is new and no signatures changed. Output for parseable entries matches the old `render_index` byte for byte. The only difference is that ties on case-insensitive titles are now broken deterministically, where the old order came from HashMap order. Frozen surfaces are untouched. The learning worker's ledger code is unchanged.

## Findings

### F1 MINOR: a carried line can survive forever when the page parses under a different id
`pk-store/src/bundle.rs:158-190`, `pk-store/src/store.rs:254-266`. A line is carried when its id is not in `parsed_ids` and `<id>.md` exists. The check compares the *frontmatter* id with the *path*.
- Scenario A: `wiki/foo.md` has frontmatter `id: bar`. The fresh line is `/bar.md`. `/foo.md` is not in `parsed_ids` and `foo.md` exists, so the old `* [Foo](/foo.md)` line is kept on every rebuild and the page is listed twice, for good.
- Scenario B: on a case-insensitive filesystem (macOS, Windows CI), an existing line `/Foo.md` with file `foo.md` and id `foo` passes `is_file()`, so it is kept forever.
- Fix: have `scan_wiki_tree` return the relative paths that failed to read or parse. Carry a line only when its exact path is in that set, instead of "exists and id not parsed".

### F2 MINOR: a multi-line title or description is silently cut, but only when something is carried
`bundle.rs:181-189`. The merge re-parses `rendered` text with `index_sections`, and that keeps only lines starting `* [`. If an LLM-written description contains `\n`, `render_index` emits it across two lines. On the merge path the second line is dropped. A newline inside a title drops the whole entry. The output then differs depending on whether anything was carried.
- Fix: build the merge from the structured groups (refactor `render_index` into a group builder plus `render_index_groups`) rather than re-parsing its output. Optionally collapse newlines in title and description.

### F3 MINOR: a new on-disk artifact in a git-tracked wiki needs a downstream handoff note
`store.rs:451, 489-506`. The change creates `wiki/.index.lock` on first use, and a crash between create and rename leaves `.index.md.<pid>.<ns>.<n>.tmp` / `.log.md.*.tmp` files behind. Neither is a `.md` file, so scan, lint and conformance ignore them (verified). But issue #15 shows consumers commit the wiki, so `git add -A` will commit the lock file and any leftover temp files.
- Fix: have `pk init` write `wiki/.gitignore` (`.index.lock`, `.*.tmp`), or document it. Include this in the task 4.1 handoff note: skill-system docs and the release-version-matrix entry. Optionally sweep stale temp files while holding the lock.

### F4 MINOR: every rebuild parses the whole wiki while holding the cross-process lock
`store.rs:255`. Before, the index came from the in-memory snapshot. Now each ingest parses every page and SHA-hashes every file while holding the exclusive lock, and concurrent `append_log` calls wait for it. The `scan_wiki_tree` comment already reports that large stores take minutes. Each ingest now pays the full open-time scan cost twice.
- Fix: use a scan that only lists ids and pages that fail to parse (no hashing, no TextIndex). Or measure this against a large wiki before release.

### F5 MINOR: process issues
- A dependency edit (`fs2` added to `pk-store/Cargo.toml` and `Cargo.lock`) landed in the `docs(openspec)` commit 937cf4c. Rule 5 makes a new crate dependency a request to knowledge-lead, even though `fs2` is already a workspace dependency. Record that approval in the PR.
- `tasks.md` boxes 1.1, 1.2 and 2.1 are still unchecked. Task 4.1 (handoff) is open.

### NITs
- N1 `bundle.rs:164-178`: carried lines with the same id but different text, or the same line under two sections, are not deduplicated. Only byte-identical lines are, so the page is listed twice.
- N2 `store.rs:262`: a non-UTF-8 `index.md` is decoded lossily, so U+FFFD is written permanently into carried lines. This is still better than the old `log.md` behaviour, where `unwrap_or_default` wiped the file.
- N3 `store.rs:489-506`: the directory is not fsynced after the rename, so on power loss the rename may not be durable. The file is still never truncated.
- N4 Windows: `std::fs::rename` over `index.md` fails with ERROR_ACCESS_DENIED if another process holds it open without FILE_SHARE_DELETE (some editors, AV). The error is surfaced, not silent. I did not exercise this here: the gate ran on darwin only.
- N5 There is no reentrancy guard. A future caller that holds `.index.lock` and calls `regenerate_index` or `append_log` would deadlock itself, because flock on a second fd blocks. Document it on `lock_index`.
- N6 RED 9e1e962 (concurrent `append_log`) depends on a race and may pass on main by chance. 5e71337 is deterministic RED. TDD order holds otherwise: both RED commits contain only tests and precede their GREEN commits. 602c856 is a post-GREEN regression test, as tasks.md 2.1 specifies. It does cross the worker boundary: it runs the real `prometheus-learning-worker run-once` binary on a schemaVersion 2 job, which reaches `store.regenerate_index` at `pk-learning-worker/src/main.rs:547`.

## Spec coverage
All three spec requirements and all four scenarios have tests (store tests plus bundle unit tests). Nothing in the diff goes beyond the spec. The CRLF, sectionless-line, reserved-name and `..` cases are covered by unit tests.

## Gate (darwin, `CARGO_TARGET_DIR=<scratchpad>/verifier-target`, run serially, no hook blocks)
| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS (exit 0) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS (exit 0) |
| `cargo test --workspace --locked` | PASS (exit 0): 168 passed, 0 failed, 0 ignored |

## Confirmation pass (602c856..3691130, 2026-10-03)

Commits reviewed: f57db3f (RED, tests only), 0e27913 (GREEN, pk-store only), 3691130 (docs). The RED commit comes before the GREEN commit.

| Finding | Status | Evidence |
|---|---|---|
| F1 | RESOLVED | `scan_wiki_tree` now records `failed_paths`, the exact path-derived fallback ids of pages that failed to read or parse (`store.rs` scan). `render_index_preserving` carries a line only when its target is in that set (`bundle.rs`). A page whose filename differs from its id, or a link that differs only in case, is no longer carried. Covered by the store test `page_whose_filename_differs_from_its_id_is_listed_once` and the unit test `preserving_never_carries_a_parsed_page_listed_under_another_id`. |
| F2 | RESOLVED | The merge is now structural: `entry_groups` produces sort keys and lines, the carried lines are added, and `render_index_groups` renders once. Rendered text is never re-parsed. Covered by `multi_line_description_survives_when_a_line_is_carried` and `preserving_keeps_multi_line_fresh_entries_when_carrying`. |
| F3 | RESOLVED | The repo `.gitignore` ignores `wiki/.index.lock` and `wiki/.*.tmp`. README and design.md tell consumers to do the same. Handoff note item 6 asks the skill system to add these patterns to its scaffolding. Temp-file sweeping is not implemented, which is acceptable. |
| F4 | DEFERRED (accepted) | Measure first. Not a merge blocker. |
| F5 | RESOLVED | tasks.md is ticked, apart from 3.1, which is correct. proposal.md:30 records the `fs2` dependency, which knowledge-lead authored and the lead's docs commit carries. The PR should still state the rule-5 approval explicitly. |
| N1 | RESOLVED | `carried_targets` keeps at most one line per target, whatever its text. Covered by `preserving_files_sectionless_lines_under_uncategorized_once_per_target`. |
| N5 | RESOLVED | Doc comments on `regenerate_index`, `append_log` and `lock_index` say the lock is not reentrant. No runtime guard, which is acceptable. |

Regression checks:
- **Byte-identical rendering:** `render_index` is `render_index_groups(entry_groups(..))`. It sorts by (lowercased real title, line) and has the same version block, sections, description trimming and placeholder as before. Ties are still broken deterministically. The sort key now comes from the real title instead of a re-parsed one, which is strictly better. `preserving_matches_render_index_when_nothing_is_carried` asserts equality, including a multi-line description.
- **Unreadable or non-UTF-8 pages:** a page whose bytes are not UTF-8 fails `read_to_string`, its path goes into `failed_paths`, and its line is carried. Correct.
- **Path normalisation:** components are joined with `/` (Windows-safe) and reserved files are skipped before recording. Traversal targets (`/../x.md`) can never match a failed path.

New NITs, none blocking:
- C1: matching is case-sensitive. If a page fails to parse and its existing index line differs in case from the filename (`/Foo.md` for `foo.md`), the line is now dropped rather than kept. pk always writes the link from the id, which equals the path, so this only affects hand-edited indexes.
- C2: if `foo.md` fails to parse while another file parses with frontmatter `id: foo`, `/foo.md` is listed twice: once fresh and once carried. This needs an id collision.
- C3: a non-UTF-8 *filename* is converted lossily to U+FFFD, so it may not match the index line. Very rare.
- The earlier N2, N3, N4 and N6 nits still stand.

### Gate (second run, same `CARGO_TARGET_DIR`, serial, darwin, no hook blocks)
| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS (exit 0) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS (exit 0, 0 warnings) |
| `cargo test --workspace --locked` | PASS (exit 0): 171 passed, 0 failed, 0 ignored. The previous run had 168: this pass adds 2 store tests and 1 net bundle unit test. |

**Final verdict: PASS.** F4 is deferred and the remaining nits are non-blocking. This is a separate-context review by the same model family; a human must review the PR before merge.
