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

### Requirement: `sources` is a list of mappings that round-trips without loss
A source SHALL be a mapping with a required `resource` and an optional `id`, and SHALL preserve every other key it was read with. The writer SHALL emit the mapping form only. The reader SHALL accept a legacy string as `{ resource: <string> }`.

#### Scenario: v0.2 credibility signals survive a round trip
- **WHEN** a document whose source carries `id`, `resource`, `title`, `author`, `usage_count` and `last_modified` is parsed and written back
- **THEN** all six keys are present with their original values

#### Scenario: A legacy string source is upgraded on write
- **WHEN** a document with `sources: ["session:abc-123"]` is parsed and written back
- **THEN** the output has `sources: [{ resource: "session:abc-123" }]`

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

#### Scenario: A nested index carries no frontmatter
- **WHEN** an `index.md` is rendered for a subdirectory
- **THEN** it has no frontmatter block

### Requirement: The behaviour holds on every platform pk builds for
Every scenario above SHALL pass on `ubuntu-latest`, `macos-latest` and `windows-latest`, and a CRLF document SHALL parse to the same sources and the same `generated` as its LF twin.

#### Scenario: CRLF does not change provenance
- **WHEN** a v0.2 document is parsed once with LF and once with CRLF line endings
- **THEN** the two entries have equal `sources` and equal `updated_at`
