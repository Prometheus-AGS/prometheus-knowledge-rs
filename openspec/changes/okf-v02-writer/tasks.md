Test-first throughout. Cheap checks while implementing (`cargo check -p <crate>`, the one test); the full battery in §6.

## 1. The source type

- [x] 1.1 In `pk-core/tests/types_tests.rs`, add tests: a bare string deserialises to a source with that `resource`; a mapping keeps `id`, `title`, `author`, `usage_count`, `last_modified`; a mapping with no `resource` is an error; `with_sources` still accepts strings.
- [x] 1.2 Add `Source` to `pk-core/src/types.rs`, change `WikiEntry.sources` to `Vec<Source>`, and export it from `pk-core/src/lib.rs`.
- [x] 1.3 `cargo check --workspace`; fix the two other users named in the proposal (`pk-librarian/src/librarian.rs`, `pk-learning-worker/src/main.rs`).

## 2. Frontmatter

- [x] 2.1 In `pk-store/src/markdown.rs` tests: a written entry has `generated.by` starting `pk/`, `generated.at` equal to `updated_at`, and no `timestamp`; a read `generated.by` survives a write; `timestamp` alone still sets `updated_at`; `generated.at` outranks an older `timestamp`; a document that already carries `generated` does not emit it twice; a CRLF v0.2 document equals its LF twin in `sources` and `updated_at`.
- [x] 2.2 Implement in `pk-store/src/markdown.rs`: a typed `generated`, `sources` as `Vec<Source>`, `timestamp` read-only.
- [x] 2.3 Mutation: restore the `timestamp` write and confirm 2.1 fails.

## 3. Citations

- [x] 3.1 In `pk-librarian` tests: the compile system prompt does not contain `Citations`; two sources reducing to one slug get distinct ids; every `[^label]` in a compiled body matches one source id.
- [x] 3.2 Update `pk-librarian/src/prompts.rs` and the id derivation in `pk-librarian/src/librarian.rs`.

## 4. The root index

- [x] 4.1 In `pk-store/src/bundle.rs` tests: the root index begins with an `okf_version: "0.2"`-only block; rendering twice is stable; `okf_index_reports` accepts the form pk writes. (The "nested index" case was dropped: pk renders no nested index — `render_index` has one caller.)
- [x] 4.2 Implement in `render_index`. Its caller at `pk-store/src/store.rs:228` writes what it returns and needed no change.

## 5. Documentation

- [x] 5.1 Replace "OKF v0.1" in doc comments with the version and section each actually cites; bump the workspace version.

## 6. Verify and hand over

- [x] 6.1 `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked --no-fail-fast`.
- [x] 6.2 Against a copy of a real v0.1 KB: `pk list` and `pk context` succeed before any write; after one `pk ingest`, only the touched entry is in v0.2 form.
- [ ] 6.3 **Not before section 7 is complete.** **Stop. Pushing to this repository needs the operator's go-ahead.** On go-ahead, open the PR and read all three OS jobs before reporting.

## 7. Fixes from the Rust audit — before the 6.3 stop

Report: `prometheus-skills-mini/.kbd-orchestrator/phases/karpathy-logs-node/review/okf-v02-writer/auditor-report.md`. Each fix is RED commit then GREEN commit, with a mutation.

- [x] 7.1 **CRITICAL — 1.9.0 rejects every non-empty 1.8.0 prompt snapshot.** RED first: a fixture snapshot in the 1.8.0 shape (string `sources`, no `generated_by`), with its true generation hash, must be read by `read_prompt_snapshot`. Fix in `pk-store/src/prompt_snapshot.rs`: validate identity and `byte_count` against the **stored bytes** of `entries` (`serde_json::value::RawValue`), not a re-serialisation — a stricter integrity check that does not depend on how today's struct happens to serialise. Add the scenario to `spec.md`. Reproduced on the real global snapshot: 1.8.0 → 4 candidates; this branch → 0 and a validation failure.
- [ ] 7.2 **HIGH — a malformed `generated` fails the whole document.** RED first, one test per shape 1.8.0 read: a string, a bool, a mapping with no `by`, and an `at` chrono rejects (`2026-07-01T10:00+00:00`, no offset, date only). Each must parse, with `generated` treated as absent and `updated_at` falling through to `timestamp` or now. Hold `generated` as a YAML value and take `by`/`at` only when they are strings in a mapping. Add the scenario to `spec.md`.
- [ ] 7.3 **MEDIUM — a non-string scalar in `sources` fails the document.** RED first: `sources: [12345]`, a bool, from YAML and JSON. A number or bool becomes its string form; `null` and nested sequences give an error that names `sources`. 1.8.0's `Vec<String>` read these.
- [ ] 7.4 **HIGH — a de-duplication suffix can take a label the model cited with**, plus the LOW in the same function. RED first: `[{id:x,A},{id:x,B},{id:x-2,C}]` with the body citing `[^x-2]` must leave C as `x-2`. Reserve the first occurrence of every distinct model label before suffixing any repeat, compare labels case-folded (markdown footnote labels are case-insensitive), and replace the cubic `Vec` scan and the `expect` with a `HashSet` and a plain loop.
- [ ] 7.5 **MEDIUM — `is_footnote_label` rejects valid labels, so pk causes the mismatch.** RED first: `notes.md`, `session:abc`, `a/b` are kept verbatim and the body's footnote still matches. Accept any label with no whitespace and none of `[`, `]`, `^`.
- [ ] 7.6 **MEDIUM — a footnote definition whose text is an absolute `.md` path becomes a link-graph edge.** RED first: a body with `[^notes]: /abs/path/file.md` yields no link. Parse with `Options::ENABLE_FOOTNOTES` in `extract_body_links` (`pk-store/src/bundle.rs`). Unchanged code, but the new prompt is what produces this input.
- [ ] 7.7 **MEDIUM, DECLINED with a reason — `Source.extra` is `pub`, so code could re-insert `id`/`resource` and serialise duplicate keys.** Unreachable from parsed data and nothing in the workspace does it; no failure has been observed, so no guard is added. Record the decision in `design.md` and leave it for the day a caller needs to build a `Source` with extras.
- [ ] 7.8 **MEDIUM — the MCP `get` tool's wire shape changed.** `handle_get` returns the whole `WikiEntry`: `sources` is now an array of mappings and `generated_by` is new. Also a source-breaking public API change (`WikiEntry.sources` type, a new public field) inside a minor bump. State both under **BREAKING** in `proposal.md` and in the PR body. No code.
- [ ] 7.9 Re-run the 6.1 battery. Redo 6.2 with a copy of a real **1.8.0 prompt snapshot** in the sandbox, so `pk context` is exercised before any write against data that exists — the check 6.2 made vacuous. Update `evidence.md`, including why the first version missed finding 1.
