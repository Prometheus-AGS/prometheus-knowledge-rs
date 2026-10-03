## Lock

An exclusive `fs2` lock on `wiki/.index.lock`, blocking, held across scan, merge, render and write. Blocking (not skipping) is right here: a skipped rebuild would leave a stale index, while the wait is bounded by one scan and one write. The lock file is not a concept page: it lacks the `.md` extension, so `scan_wiki_tree` ignores it.

## Fresh scan

Rendering calls `scan_wiki_tree(&self.wiki_dir)` inside the lock. The in-memory store is not updated from it; only the index is derived from disk. Two processes that opened the store before either upserted therefore both end up listed, whichever writes last.

## Preserving unparseable pages

The existing `index.md` is read inside the lock. An entry line `* [Title](/id.md)…` is carried into the new index, verbatim, under its existing `## Section` when:
- its target file `wiki/<id>.md` exists on disk, and
- `<id>` is not among the entries this binary parsed.

Parsed entries are always rendered fresh from their frontmatter, so they never duplicate a carried line. Entries whose target file no longer exists are dropped, which is how deletion works. The merge is a pure function in `bundle.rs`, testable without a store.

## Atomic writes

Write to a uniquely named temporary file in `wiki/`, sync it, then rename over the target. A killed process leaves either the old file or the new one, never a truncated one. `log.md` gets the same treatment because `append_log` has the same exposure.

## What this cannot fix

A binary older than this change (such as the deployed 1.8.0 worker) still rewrites the index destructively. The remedy for that is operational: same-version installs, tracked in the skill-system handoff note.
