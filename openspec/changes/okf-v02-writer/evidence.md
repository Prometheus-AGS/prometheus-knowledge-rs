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

---

# Addendum — after the Rust audit (section 7)

## Why the first version of §6.2 missed a CRITICAL

§6.2 above copied only the wiki root into its sandbox, to limit what `pk ingest` sent to the model. So no
1.8.0 prompt snapshot was ever read, and "`pk context` before any write → no output" — labelled vacuous
there — was vacuous in exactly the way that hid the defect: **1.9.0 rejected every non-empty snapshot 1.8.0
had written.** An independent report-only audit found it; it was then reproduced read-only on the real
global snapshot (1.8.0 → 4 candidates; the branch → 0 and `failed identity or count validation`). All 136
tests passed with the defect present.

## 7.9 Full battery, after the fixes

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| `cargo test --workspace --locked --no-fail-fast` | **151 passed, 0 failed, 0 ignored** |

136 + 15 added by section 7: 4 (snapshot) + 2 (`generated`) + 2 (`Source`) + 5 (labels) + 2 (link extraction).

## 7.9 The §6.2 check, redone so it cannot be vacuous

A sandbox **home** this time: the wiki-root files plus a copy of the real `.prompt-snapshots/global` written
by pk 1.8.0 on 2026-08-03 (one generation, `cc1d204a…`, 4 entries). Both binaries run with `HOME` pointing
at it. The real knowledge base and its snapshots were never written: the wiki root still matches its
pre-test checksums and the newest real generation is still dated Aug 3.

**Before any write — reading data that exists**

| | generation | candidates | failures |
|---|---|---|---|
| pk 1.8.0 | `cc1d204a…` | 4 | none |
| pk 1.9.0 (this branch) | `cc1d204a…` | 4 | none |

Identical, and the sandbox was byte-identical after both reads. On the real KB the same holds for the
`shared` scope: 128 candidates under both versions.

**One real ingest with 1.9.0**, then `pk snapshot --scope global`: a new generation `18983af1…` with 5
candidates. The 1.8.0 generation file is still present and still byte-identical. (The ingest alone committed
no global snapshot — pk seeds only the project scope on ingest. Pre-existing behaviour, not this change.)

**After — and the direction this change cannot fix**

| | reads the 1.9.0 snapshot | lists the mixed wiki |
|---|---|---|
| pk 1.9.0 | 5 candidates | 5 entries |
| pk 1.8.0 | **0 candidates** — `invalid type: map, expected a string` | **4 entries** — `skipping malformed entry`, then `article not found` for the new one |

**A knowledge base that 1.9.0 has written to cannot be shared with a 1.8.0 binary.** 1.8.0 silently drops
every v0.2 entry and fails every snapshot 1.9.0 commits. This is inherent in OKF v0.2's mapping-form
`sources` — 1.8.0 holds them as strings — so it is not fixable from this side. Upgrading is all-or-nothing
per knowledge base. It is stated under BREAKING in `proposal.md`.

## Mutations run in section 7, and two that taught something

Every fix was RED commit then GREEN commit with at least one mutation. Two are worth recording:

- Three mutants of the snapshot fix first ran **silently**: a `grep` for `^error\[` hid their compile errors,
  so there was no telling whether they were killed or never built. They were re-run with exact-string
  replacements and visible output before being counted.
- A mutant of the label code **survived**: comparing a *suffixed* label case-sensitively broke no test and
  produced duplicate ids (`doc-2`, `Doc`, `Doc`). A test was added that kills it.
