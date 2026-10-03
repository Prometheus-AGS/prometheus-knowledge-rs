---
type: SessionRecord
id: karpathy-session-d254536092251d2d
title: Karpathy session d25453609225
tags:
- karpathy
- session-learning
sources:
- resource: session:6cef7a5e-4309-4ef9-91eb-79c07e58536e
generated:
  by: pk/1.9.0
  at: 2026-10-03T20:09:32.018686+00:00
created_at: 2026-10-03T20:09:32.018686+00:00
updated_at: 2026-10-03T20:09:32.018686+00:00
revision: 0
---

## Delta

PR #27's branch is deleted and the learning worker is reinstalled.

**Branch:** I deleted `fix/preserve-wiki-index` locally and on GitHub. `main` is the only branch again, up to date at `5bfdc88`, and issue #15 closed when the PR merged.

**Worker:** `~/.local/bin/prometheus-learning-worker` now reports **1.9.0**, built from `main` with the fix. The old 1.8.0 binary is kept as `~/.local/bin/prometheus-learning-worker.1.8.0.bak.20261003150858`.
- I put the new binary in place in one atomic step. The `ai.prometheus.learning-worker` LaunchAgent wasn't running at that moment, so its next run uses the new binary.
- That run rebuilds this repo's `index.md` from the pages on disk, restoring the `okf_version` block and the dropped entries. I haven't checked that rebuild yet.
- The queue looks healthy: 0 stale and 0 stalled memory operations. One job shows as processing, probably left by an interrupted earlier run; the worker recovers those at the start of its next run.

**The tier-guard hook's one-command opt-in doesn't work.** `PROMETHEUS_TIER3=1 <command>` is ignored, because the hook checks its own environment before the command runs, not the command's. I used the hook's other documented opt-in instead: I created `.kbd-orchestrator/tier3.allow` for the build and deleted it right after, and confirmed it's gone. That's two hook defects found today, with the text-matching false positive from earlier, both belonging to the bootstrap skill that installs this hook.

**`pk` and `pk-cherry` still predate the fix.** Both were built on September 21. `pk` says 1.9.0, so the version mismatch is gone, but `pk ingest` still runs the old index code: no lock, the old rebuild that drops unparseable pages, and in-place writes. A `pk ingest` racing a worker run could still lose entries. Rebuilding both from `main` the same way would make every writer on this machine use the fix. Want me to?

## Root Cause

No explicit root-cause section was captured; preserve this as a session record, not an inferred diagnosis.

## Corrective Actions

Review and promote only reusable findings.

## Session Metadata

- Harness: claude-code
- Session: 6cef7a5e-4309-4ef9-91eb-79c07e58536e
- Captured: 2026-10-03T20:09:26.335442Z
- Project: /Users/gqadonis/Projects/prometheus/prometheus-knowledge

## Changed Paths

- No changed paths detected.
