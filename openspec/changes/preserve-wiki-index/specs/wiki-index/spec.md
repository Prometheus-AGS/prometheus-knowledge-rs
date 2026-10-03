## Purpose

Keeps the OKF v0.2 bundle-root `index.md` complete: rebuilding it never drops an entry whose page still exists, never drops the `okf_version` declaration, and never leaves a truncated file.

## ADDED Requirements

### Requirement: Rebuilding the index preserves pages this binary cannot parse
When the index is rebuilt, an existing index entry whose target page exists on disk but cannot be parsed SHALL be kept under its existing section.

#### Scenario: Page written in a format this binary does not understand
- **WHEN** `index.md` lists a page whose frontmatter this binary fails to parse, and the index is rebuilt
- **THEN** that entry is still listed, under the same section heading

#### Scenario: Page deleted from disk
- **WHEN** `index.md` lists a page whose file no longer exists, and the index is rebuilt
- **THEN** that entry is no longer listed

### Requirement: Rebuilds reflect every page on disk
The index SHALL be rendered from the pages on disk at the time of the rebuild, not from the set known when the process started, and concurrent rebuilds SHALL be serialized.

#### Scenario: Two writers opened before either ingested
- **WHEN** two processes open the same wiki, each ingests a different page, and each rebuilds the index
- **THEN** the final index lists both pages

### Requirement: The index keeps its version declaration and is never truncated
Every rebuilt index SHALL begin with the `okf_version: "0.2"` frontmatter block, and `index.md` and `log.md` SHALL be replaced atomically.

#### Scenario: Rebuild of an index that declares its version
- **WHEN** the index is rebuilt
- **THEN** it still begins with the `okf_version` block, followed by `# Wiki Index`
