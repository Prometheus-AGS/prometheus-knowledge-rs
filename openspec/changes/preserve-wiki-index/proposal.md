## Why

`pk ingest` and the learning worker can replace `wiki/index.md` with a version that has no `okf_version` frontmatter and only the newest page type's section. Every other entry is dropped, though its page still exists (GitHub issue [#15](https://github.com/Prometheus-AGS/prometheus-knowledge-rs/issues/15), about 90 `## Reference` entries lost in one project).

The trigger on the reporter's setup, and on the maintainer's machine, is version skew: `pk` is 1.9.0 while `prometheus-learning-worker` is 1.8.0, installed by the skill system from its 1.8.0 submodule pin. A 1.8.0 binary cannot parse pages written in OKF v0.2's mapping-form `sources` (1.9.0 changed `WikiEntry.sources` from `Vec<String>` to `Vec<Source>`). `scan_wiki_tree` skips those pages as parse failures, and the 1.8.0 `render_index` writes no frontmatter, so its rebuild overwrites the index with only the pages it can read.

The code on `main` still allows the same loss:
- **Parse failures vanish.** `MarkdownStore::regenerate_index` renders only from parsed entries.
- **Stale snapshot.** It renders from the snapshot taken when the process opened the store, so overlapping ingests drop each other's pages.
- **Non-atomic write.** It writes in place, so a run killed mid-write can truncate the file.

## What Changes

- `regenerate_index` holds an exclusive advisory lock (`wiki/.index.lock`) for the whole read, render and write, so concurrent writers serialize.
- It renders from a fresh scan of the wiki tree inside the lock, not from the process's open-time snapshot.
- Index entries whose target page still exists on disk but cannot be parsed by this binary are preserved in their existing section. Entries whose page is gone are dropped.
- `index.md` and `log.md` are written atomically (temporary file, then rename).

## Capabilities

### New Capabilities
- `wiki-index`: maintaining the OKF v0.2 bundle-root `index.md` without losing entries or frontmatter.

### Modified Capabilities
(none)

## Impact

- **Code**: `pk-store/src/store.rs` (`regenerate_index`, `append_log`) and `pk-store/src/bundle.rs` (a pure merge helper beside `render_index`).
- **Dependencies**: `pk-store` gains `fs2`, already a workspace dependency used by `pk-learning-worker`. No new crate enters `Cargo.lock`.
- **Contracts**: no frozen surface changes. The `index.md` format is unchanged, `regenerate_index` keeps its signature, and `pk-cli` and `pk-learning-worker` call it as before. No semver impact.
- **Consumers**: the fix only protects KBs once every writer is at least this version. The skill system must bump its 1.8.0 pin and install `pk`, `pk-cherry` and the worker from one commit; see `.agent-team/prometheus-knowledge-maintainers/handoffs/issue-15-skill-system.md`.
- **GitHub**: closes issue #15.
