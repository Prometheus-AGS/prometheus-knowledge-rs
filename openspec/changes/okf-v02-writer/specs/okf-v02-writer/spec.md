## ADDED Requirements

### Requirement: The writer emits `generated`, not `timestamp`
`entry_to_markdown` SHALL write `generated` as a mapping with `by` and `at`, where `by` is `pk/<crate version>` and `at` is the entry's `updated_at` in RFC 3339, and SHALL NOT write a `timestamp` key.

#### Scenario: A written entry carries generated
- **WHEN** an entry is serialised
- **THEN** its frontmatter has `generated.by` beginning `pk/` and `generated.at` equal to `updated_at`, and has no `timestamp` key

#### Scenario: A producer's own generated.by is not overwritten
- **WHEN** an entry read from disk carries `generated.by: librarian/some-model` and is written back unchanged
- **THEN** `generated.by` is still `librarian/some-model`

### Requirement: The reader accepts v0.1 and v0.2 timestamps
`markdown_to_entry` SHALL take `updated_at` from an explicit `updated_at`, else from `generated.at`, else from a legacy `timestamp`, else from the current time.

#### Scenario: A v0.1 entry still parses
- **WHEN** a document carries `timestamp` and neither `generated` nor `updated_at`
- **THEN** it parses, and `updated_at` equals that timestamp

#### Scenario: generated.at outranks a stale timestamp
- **WHEN** a document carries both `generated.at` and an older `timestamp`
- **THEN** `updated_at` equals `generated.at`

### Requirement: A document 1.8.0 could read does not fail because of `generated`
pk 1.8.0 did not model `generated`, so a document carrying any shape of it parsed. The reader SHALL treat a `generated` that is not a mapping with a string `by` as absent, and SHALL let a `generated.at` it cannot parse fall through to the next source of `updated_at`. `updated_at` and `timestamp` SHALL stay as strict as 1.8.0 had them.

#### Scenario: A malformed generated is absent, not fatal
- **WHEN** `generated` is a string, a boolean, a sequence, a mapping with no `by`, or a mapping whose `by` is not a string
- **THEN** the document parses, its body is intact, and it carries no `generated_by`

#### Scenario: An unparseable generated.at falls through
- **WHEN** `generated.at` has no offset, is a date alone, omits the seconds, or is not a date, and the document also carries a `timestamp`
- **THEN** the document parses, `updated_at` equals the `timestamp`, and `generated.by` is still read

### Requirement: `sources` is a list of mappings that round-trips without loss
A source SHALL be a mapping with a required `resource` and an optional `id`, and SHALL preserve every other key it was read with. The writer SHALL emit the mapping form only. The reader SHALL accept a legacy string as `{ resource: <string> }`.

#### Scenario: v0.2 credibility signals survive a round trip
- **WHEN** a document whose source carries `id`, `resource`, `title`, `author`, `usage_count` and `last_modified` is parsed and written back
- **THEN** all six keys are present with their original values

#### Scenario: A legacy string source is upgraded on write
- **WHEN** a document with `sources: ["session:abc-123"]` is parsed and written back
- **THEN** the output has `sources: [{ resource: "session:abc-123" }]`

#### Scenario: A scalar source that is not text is read as its text
- **WHEN** a document written for 1.8.0 lists a number or a boolean under `sources`, in YAML or in JSON
- **THEN** it loads, with the scalar's text as the `resource`, as 1.8.0 loaded it

#### Scenario: A source without a resource is rejected
- **WHEN** a document carries a source mapping with no `resource`
- **THEN** parsing fails with a frontmatter error naming the missing key

### Requirement: Claims are cited by keyed footnotes
The compile prompt SHALL ask for `[^id]` footnotes whose labels are `sources[].id`, SHALL NOT ask for a `# Citations` section, and every source pk attaches to a compiled entry SHALL carry an `id` that is unique within the entry. This is deliberately stricter than OKF §5.1, where `id` is optional and only SHOULD be present when cited: pk always cites, and a duplicate label misattributes silently.

#### Scenario: The prompt no longer requests the superseded form
- **WHEN** the compile system prompt is read
- **THEN** it does not contain the word `Citations`

#### Scenario: Source ids are unique within an entry
- **WHEN** a compile response lists two sources whose text reduces to the same slug
- **THEN** the entry's two sources have different `id` values, and each footnote label in the body matches exactly one of them

### Requirement: The bundle root declares OKF 0.2
The bundle-root `index.md` SHALL begin with a frontmatter block containing only `okf_version: "0.2"`, and regenerating the index SHALL keep it.

#### Scenario: A regenerated index keeps its declaration
- **WHEN** the index is rendered twice
- **THEN** both outputs begin with the `okf_version: "0.2"` block, and the second equals the first

#### Scenario: pk writes no index but the root one
- **WHEN** the callers of `render_index` are listed
- **THEN** there is exactly one, and it writes `<wiki root>/index.md` — so every index pk writes is the bundle root's, and no index below the root is written with or without frontmatter

#### Scenario: The writer and the linter agree
- **WHEN** the index pk renders is passed to pk's own index structure check, for an empty and a non-empty wiki
- **THEN** the check reports nothing

### Requirement: A change to how an entry serialises does not invalidate stored snapshots
`read_prompt_snapshot` SHALL verify a snapshot's generation and byte count against the entries as they are stored in the file, and SHALL NOT derive them by re-serialising the deserialised entries.

#### Scenario: A snapshot written by 1.8.0 still validates
- **WHEN** a snapshot whose entries carry string-form `sources` and no `generated_by` key, with the generation 1.8.0 computed for it, is read by 1.9.0
- **THEN** it validates, its entries load with each string source as a `resource`, and `pk context` returns the same candidates 1.8.0 returned

#### Scenario: Stored entries that differ from what was hashed are still refused
- **WHEN** one character inside a stored entry is changed
- **THEN** the snapshot fails identity validation

#### Scenario: Formatting is not content, and content is not formatting
- **WHEN** pretty-printed JSON containing strings with runs of spaces, an escaped quote, a backslash and an escaped newline is compacted
- **THEN** the result equals `serde_json`'s compact serialisation of the same value, byte for byte

### Requirement: The behaviour holds on every platform pk builds for
Every scenario above SHALL pass on `ubuntu-latest`, `macos-latest` and `windows-latest`, and a CRLF document SHALL parse to the same sources and the same `generated` as its LF twin.

#### Scenario: CRLF does not change provenance
- **WHEN** a v0.2 document is parsed once with LF and once with CRLF line endings
- **THEN** the two entries have equal `sources` and equal `updated_at`
