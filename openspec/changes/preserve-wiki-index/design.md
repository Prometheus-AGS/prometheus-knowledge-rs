## Lock

An exclusive `fs2` lock on `wiki/.index.lock`, blocking, held across scan, merge, render and write. Blocking (not skipping) is right here: a skipped rebuild would leave a stale index, while the wait is bounded by one scan and one write. The lock file is not a concept page: it lacks the `.md` extension, so `scan_wiki_tree` ignores it.

## Fresh scan

Rendering calls `scan_wiki_tree(&self.wiki_dir)` inside the lock. The in-memory store is not updated from it; only the index is derived from disk. Two processes that opened the store before either upserted therefore both end up listed, whichever writes last.

## Preserving unparseable pages

The existing `index.md` is read inside the lock. The scan reports the bundle-relative paths of the pages it could not read or parse. An entry line `* [Title](/<path>.md)…` is carried into the new index, verbatim, under its existing `## Section`, only when `<path>` is exactly one of those failed paths.

Matching on the failed path, not on "not among the parsed ids", matters. A page whose filename differs from its frontmatter id (`foo.md` declaring `id: bar`), or whose link differs only in case on a case-insensitive filesystem, parses and is rendered fresh. Its old line must not be carried as well, or the page would be listed twice forever (verifier finding F1).

Lines for pages that parse, or whose file is gone, are dropped. At most one line is carried per target. Rendering happens once, from the parsed entries plus the carried lines grouped by section (`bundle::render_index_preserving`). It never re-parses rendered text, which would lose entries whose title or description spans lines (finding F2). Parsed entries render byte-identically to `render_index`.

## Atomic writes

Write to a uniquely named temporary file in `wiki/`, sync it, then rename over the target. A killed process leaves either the old file or the new one, never a truncated one. `log.md` gets the same treatment because `append_log` has the same exposure.

## Lock scope

`append_log` holds the same lock across its read-modify-write, so concurrent appends cannot lose a line. Neither method may be called while the caller already holds the lock.

## Consumer housekeeping

Projects that commit their wiki should ignore `wiki/.index.lock` and the temporary files an interrupted write leaves (`wiki/.*.tmp`).

## What this cannot fix

A binary older than this change (such as the deployed 1.8.0 worker) still rewrites the index destructively. The remedy for that is operational: same-version installs, tracked in the skill-system handoff note.
