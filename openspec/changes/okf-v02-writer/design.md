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

The compile response is JSON from the model (`librarian.rs`, `parse_compile_response`). §5.1's footnote
join is by label, and a duplicate label misattributes silently, so every source gets an id unique within
the entry.

**The model chooses the labels; pk does not re-derive them.** An earlier draft of this section had pk slug
every id from the source text and "tell the model which ids exist". That cannot work: the model *discovers*
the sources, so pk has nothing to tell it in advance, and re-deriving a label the model already cited with
(`ga4_schema` → `ga4-schema`) would leave the body's `[^ga4_schema]` matching nothing. So the prompt asks
for `{id, resource}` objects and footnotes that use those ids, and pk:

1. keeps a label verbatim when it can be a footnote label (ASCII letters, digits, `-`, `_`);
2. derives one from `resource` only when the label is missing or unusable — a bare-string source from a
   model that ignored the schema still parses, because `Source` reads both shapes;
3. de-duplicates with `-2`, `-3`…, the first keeping the label.

Step 1 runs as a pass of its own **before** step 2. Found by reasoning about orderings the first tests did
not cover, then reproduced with a failing test: in a single pass, a label derived for an *uncited* source
earlier in the list took `x`, the source the model had labelled `x` and cited became `x-2`, and the
body's `[^x]` resolved to the wrong source — exactly the failure this code exists to prevent.

A footnote whose label matches no source is left as the model wrote it. No failure of that kind has been
observed, and inventing a repair for it would be guessing at the model's intent.

