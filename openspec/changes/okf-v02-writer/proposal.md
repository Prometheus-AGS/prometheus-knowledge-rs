## Why

pk writes Open Knowledge Format v0.1. OKF v0.2 (2026-07-25) supersedes two v0.1 forms (§13.1): `timestamp` becomes `generated: { by, at }`, and the body `# Citations` list becomes `sources` in frontmatter, cited by `[^id]` footnotes. `prometheus-skills-mini` vendors pk as its only OKF writer and is bound to v0.2 by its operator, so pk must emit the v0.2 forms.

Precision about what is wrong today: by §11 a bundle is v0.2-conformant with nothing but parseable frontmatter and a `type`, so what pk writes is already *conformant*. It is not *current*: it emits both superseded forms, and its `sources` is a list of strings where v0.2 defines a list of mappings with a required `resource`.

## What Changes

- The writer emits `generated: { by: pk/<version>, at: <updated_at> }` and no longer emits `timestamp`.
- `sources` is written as a list of mappings, `{ id, resource, … }`. **BREAKING for readers of pk's own files that expect strings** — none exist outside pk.
- The reader accepts both shapes: a legacy string becomes `{ resource: <string> }`; a mapping keeps every key it carries, so `title`, `author`, `usage_count` and `last_modified` survive a round trip. `timestamp` is read when `generated` is absent.
- The librarian prompt stops asking for a `# Citations` body section and asks for `[^id]` footnotes whose labels are `sources[].id`.
- The bundle-root `index.md` declares `okf_version: "0.2"`, and rewriting the index preserves the declaration.

## Capabilities

### New Capabilities
- `okf-v02-writer`: what pk emits and what it accepts, stated against OKF v0.2 §4, §5, §12 and §13.

### Modified Capabilities
<!-- none: openspec/specs/ is empty in this repository -->

## Impact

- `pk-core/src/types.rs` (`WikiEntry.sources`), `pk-store/src/markdown.rs` (frontmatter), `pk-store/src/bundle.rs` (index), `pk-librarian/src/prompts.rs` and its response parsing, and the two other places that touch `sources`: `pk-librarian/src/librarian.rs` (the compile response, `:296-303`) and `pk-learning-worker/src/main.rs`. `pk-cli` and `pk-mcp` do not touch the field (`grep -rln 'with_sources\|\.sources'`, non-test sources: 4 files).
- Existing knowledge bases keep working unread-modified: a v0.1 entry parses, and is rewritten in v0.2 form the next time it is upserted.

## Non-goals

- Bulk-migrating existing entries. They migrate on their next write.
- `verified`, `status`, `stale_after`, `usage_window`, or Attested Computation: all optional in v0.2, none produced by pk today, and unknown keys are already preserved through `extra`.
- Parsing a legacy `# Citations` body into `sources`. §13.1 makes that a MAY; the body is left as written.
- Lint warnings for superseded forms.
