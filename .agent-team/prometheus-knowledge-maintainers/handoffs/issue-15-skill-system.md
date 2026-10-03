# Handoff: issue #15 to prometheus-skill-system

**From:** prometheus-knowledge-maintainers / knowledge-lead
**To:** prometheus-skill-system maintainers (Prometheus-AGS/prometheus-skill-system)
**Change:** OpenSpec `preserve-wiki-index`, closing prometheus-knowledge-rs#15
**Date:** 2026-10-03

## Why you need to act
Issue #15 (wiki `index.md` rewritten destructively) is triggered by **mixed binary versions on one knowledge base**. On affected machines `pk` is 1.9.0, but `prometheus-learning-worker` is 1.8.0. Both are installed by `scripts/install-binaries.sh` from the `tools/prometheus-knowledge` submodule, which is pinned at 1.8.0 (`01a1dbe`).

A 1.8.0 binary cannot parse pages written in OKF v0.2 mapping-form `sources`. When it rebuilds the index, it writes only the pages it can read, with no `okf_version` frontmatter. Every other entry disappears from `index.md`, although the page files remain.

prometheus-knowledge now rebuilds the index from disk under a lock and keeps entries for pages it cannot parse. That protects a KB only once **every** writer runs the fixed version.

## Requested changes (in your repository)
1. **Bump the submodule** `tools/prometheus-knowledge` to the merge commit of the preserve-wiki-index PR (or later).
2. **Install from one commit:** build `pk`, `pk-cherry` and `prometheus-learning-worker` together from that one submodule commit in `install-binaries.sh`, and refuse a partial install. Mixed versions corrupt shared KBs.
3. **Doctor check:** add one to `prometheus doctor` that fails when the three installed binaries report different versions (`<bin> --version`).
4. **Release matrix:** update the `config/release-version-matrix.json` exemption that still records prometheus-knowledge at 1.8.0.
5. **Tools reference:** in `docs/guide/13-tools-reference.md`, note that `index.md` and `log.md` are now written atomically under `wiki/.index.lock`.
6. **Ignore rules:** projects that commit their wiki should ignore `.prometheus/knowledge/wiki/.index.lock` and the temporary files left by an interrupted write (`wiki/.*.tmp`). If your project scaffolding writes a `.gitignore`, add those two patterns.

## Not requested
- No change to the MCP tools, port 8942, `pk context` / `pk ingest`, the queue layout or `status.json`.
- No change to forge-rs or prometheus-cli code. Note that their pk-core, pk-librarian and pk-store git dependencies are still locked at the initial release.

## Recovering damaged indexes
Once the fixed binaries are installed, the next ingest or worker run rebuilds `index.md` from the pages on disk, restoring dropped entries and the `okf_version` block. No manual merge is needed. The reporter's snapshot-and-merge workaround can then be removed.
