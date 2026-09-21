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

- [ ] 4.1 In `pk-store/src/bundle.rs` tests: the root index begins with an `okf_version: "0.2"`-only block; rendering twice is stable; a nested index has no frontmatter; `okf_index_reports` still accepts the root form.
- [ ] 4.2 Implement in `render_index` and its caller at `pk-store/src/store.rs:228`.

## 5. Documentation

- [ ] 5.1 Replace "OKF v0.1" in doc comments with the version and section each actually cites; bump the workspace version.

## 6. Verify and hand over

- [ ] 6.1 `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked --no-fail-fast`.
- [ ] 6.2 Against a copy of a real v0.1 KB: `pk list` and `pk context` succeed before any write; after one `pk ingest`, only the touched entry is in v0.2 form.
- [ ] 6.3 **Stop. Pushing to this repository needs the operator's go-ahead.** On go-ahead, open the PR and read all three OS jobs before reporting.
