## Why

Repeated ingest of the same (or near-identical) raw content produces a new `type: Reference` wiki entry every time instead of updating the existing one, because entry identity is derived solely from the LLM-synthesized title (`ArticleId::from_slug(title)`), which is paraphrased slightly differently on each call over the same content. Confirmed in production: 153 duplicate files across 16 KBD phases in one project's wiki (GitHub issue [#7](https://github.com/Prometheus-AGS/prometheus-knowledge-rs/issues/7)). The system already surfaces the near-duplicate entries as `related_entries()` context but never acts on that signal to prevent the duplicate write.

## What Changes

- Add a normalized-content-hash computed from the raw ingested content (independent of any LLM-synthesized title wording), used to detect that an incoming document is a repeat of previously-ingested content.
- Before `Librarian::compile()` commits a freshly-parsed `WikiEntry` as new, check the incoming content against existing entries (via the content hash, and/or a TF-IDF similarity threshold on the existing `related_entries()` results) and, on a match, update/merge into the existing entry in place (bumping its revision) instead of minting a new `ArticleId`.
- Add regression test coverage: compiling identical (or near-identical, differently-titled) raw content twice must result in one wiki entry with `revision` incremented, not two separate entries.
- Close GitHub issue #7, referencing the fix commit, once the above is implemented and verified.

## Capabilities

### New Capabilities
- `wiki-ingest-deduplication`: Detecting and merging duplicate/near-duplicate ingested content during compile, instead of creating a new wiki entry keyed only on LLM-synthesized title wording.

### Modified Capabilities
(none — no existing `openspec/specs/` capabilities exist yet in this project; this is a net-new spec)

## Impact

- **Code**: `pk-librarian/src/librarian.rs` (`Librarian::compile()`, `parse_compile_response()`), `pk-core/src/types.rs` (`WikiEntry`, `ArticleId`, `RawDoc`), `pk-store/src/store.rs` (`MarkdownStore::upsert()`, `related_entries()`).
- **Behavior**: `pk ingest` (CLI) and `knowledge_ingest` (MCP tool) both route through `Librarian::compile()`, so both get the fix automatically.
- **No new external dependencies**: stays TF-IDF/hash-based per the project's "no vector databases" constraint; reuses the existing `pk-store` `TextIndex`.
- **GitHub**: closes issue #7 on `Prometheus-AGS/prometheus-knowledge-rs`.
