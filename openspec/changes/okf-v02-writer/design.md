# Design — okf-v02-writer

## What v0.2 actually requires of a writer

Read from the spec, not from a summary (OKF `SPEC.md`, version 0.2):

- §11 conformance needs only parseable frontmatter and a non-empty `type`. pk already meets it.
- §13.1 names the two superseded forms: `timestamp` → `generated.at`, and body `# Citations` → `sources`.
- §5.2: `generated.by` is REQUIRED within `generated` and is an actor; §7 gives agents the form `<producer>/<version>`. So `pk/<CARGO_PKG_VERSION>`.
- §5.1: within a source, `resource` is REQUIRED; `id` SHOULD be present when the body cites it; `title`, `author`, `usage_count`, `last_modified` are optional.
- §4.1: consumers SHOULD preserve unknown keys on round trip.
- §12: `okf_version` MAY be declared, only in the bundle-root `index.md`.

## `sources` becomes a type, because a string cannot round-trip

Today `WikiEntry.sources` is `Vec<String>`. Reading a v0.2 mapping into a string would drop `title`, `author` and the rest on the next write — silent data loss in a format whose point is provenance. So:

```rust
pub struct Source { pub resource: String, pub id: Option<String>, pub extra: BTreeMap<String, serde_yaml::Value> }
```

with an untagged deserialiser that accepts a bare string as `Source { resource, .. }`. The struct lives in `pk-core` beside `WikiEntry`. `with_sources` keeps accepting strings, so the learning worker's call sites compile unchanged.

## `generated.by` is only defaulted, never overwritten

pk is not always the author: the librarian compiles with a model, and a human may edit a file. If the writer stamped `pk/<version>` on every write it would erase that. So `generated` is read into the entry, and the writer supplies `pk/<version>` only when none was read. `at` always tracks `updated_at`, which is what "last meaningful change" means (§5.2).

## Footnote ids

The compile response is JSON from the model with `sources: [String]` (`librarian.rs:296`). pk derives each `id` as a slug of the source text, and disambiguates a collision with a numeric suffix, because §5.1's footnote join is by label and a duplicate label misattributes silently. The prompt tells the model which ids exist rather than letting it invent them.

## Compatibility

A v0.1 knowledge base is read as-is. Nothing is migrated in bulk: an entry is rewritten in v0.2 form on its next upsert. A KB therefore holds a mix for a while, which v0.2 consumers must tolerate by §13 and pk's own reader does.

## Risk

`extra` is a flattened catch-all on `Frontmatter`. Adding a typed `generated` field changes which keys land in `extra`; a document that already carries `generated` as an unknown key today must not end up with it twice. A round-trip test on such a document guards this.
