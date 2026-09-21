# Evidence — okf-v02-writer

Recorded 2026-09-21 on macOS (darwin arm64), branch `feat/okf-v02-writer`. Self-reported: nothing here has
run on Windows or Linux yet — that is what the PR's three-OS CI is for.

## 6.1 Full battery (first workspace-wide run since the change began)

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| `cargo test --workspace --locked --no-fail-fast` | **136 passed, 0 failed, 0 ignored** (summed from every `test result` line) |

Before this change the same command gave 115. The difference, 21, is the tests this change added:
5 (`Source`) + 6 (frontmatter) + 7 (librarian) + 3 (root index).

## 6.2 Against a copy of a real v0.1 knowledge base

A sandbox copy of the wiki-root files of a real KB written by pk 1.8.0: 4 entries, each with a `timestamp`
key and string-form `sources`, plus `index.md` and `log.md`. Only the wiki root was copied, because
`pk ingest` sends related entries to the model as context. The original was never written to, and still
matched its pre-test SHA-256 checksums afterwards.

**Before any write**
- `pk --kb-dir <copy> list` → exit 0, `4 entries`. Every v0.1 entry parses under 1.9.0, including its
  string `sources`.
- `pk --kb-dir <copy> context "…" --scope project` → exit 0, **no output**. This is true but vacuous:
  the copy had no snapshot generation yet, so there was nothing to retrieve. It shows only that the
  command does not fail.
- After both reads the copy was still byte-identical to the original.

**One real ingest** — `pk ingest --source fixture:okf-v02-writer-check <note>`, compiled by `gpt-5.5`
through the local proxy → exit 0.

Files whose bytes changed (SHA-256 before and after): the new entry, `index.md`, `log.md`. Nothing else.

| | `timestamp:` | `generated:` |
|---|---|---|
| each of the 4 v0.1 entries | 1 | 0 — untouched, still v0.1 form |
| the new entry | 0 | 1 |

The new entry's frontmatter, as written:

```yaml
sources:
- id: fixture
  resource: fixture:okf-v02-writer-check
generated:
  by: pk/1.9.0
  at: 2026-09-21T22:37:27.201251+00:00
```

Its body cites with `[^fixture]` and ends with the definition line `[^fixture]: …`; it has no
`Citations` heading (`grep -c -i '^#.*citations'` → 0). The model chose the label and used it, which is
the contract the prompt now states. `index.md` opens with the `okf_version: "0.2"` block.

**After the ingest** the KB is mixed v0.1 + v0.2:
- `pk list` → `5 entries`.
- `pk context … --format json` → `candidate_count: 5`, results drawn from v0.1 entries and the new one
  together; the ingest had seeded a snapshot generation.
- `pk lint --mechanical-only --json` → exit 0, 8 warnings of two advisory kinds that pre-date this change
  and apply equally to the old entries (5 × missing `description`, 3 × orphan page). None concerns `type`,
  the index block, `generated` or `sources`.

## Not shown here

- Any platform but macOS.
- A model that ignores the new prompt and returns string sources: covered by a unit test
  (`a_source_given_as_a_bare_string_gets_a_derived_id`), not observed against a real model.
- A footnote whose label matches no source: left as written, by design; never observed.
