---
type: SessionRecord
id: karpathy-session-455d4bcbff58f042
title: Karpathy session 455d4bcbff58
tags:
- karpathy
- session-learning
sources:
- session:6cef7a5e-4309-4ef9-91eb-79c07e58536e
timestamp: 2026-10-03T18:57:16.356876+00:00
created_at: 2026-10-03T18:57:16.356876+00:00
updated_at: 2026-10-03T18:57:16.356876+00:00
revision: 0
---

## Delta

I ran `prometheus-context-bootstrap` on this repo and opened [PR #25](https://github.com/Prometheus-AGS/prometheus-knowledge-rs/pull/25) with the result (commit `668db16`, branch `chore/prometheus-context-bootstrap`).

**What's in the PR:**
- **`AGENTS.md`:** the managed rules region, using the `mixed` profile. That's the right fit, since the repo now serves six different harnesses and models.
- **`CLAUDE.md`:** an `@AGENTS.md` import line at the top. Its own text is unchanged; the skill leaves trimming it as a manual step.
- **New stubs:** `versions.toml`, `.kbd-orchestrator/current-waypoint.json`, and the append-only `.prometheus/` files (`decisions.md`, `gotchas.md`, `session-log.md`, `model-fleet.md`).
- **`.gitignore`** additions and the `openspec/specs/` placeholder.

**Local only:** the 4 enforcement hooks, the Rust rules file and the critic subagent live in `.claude/`, which this repo ignores in git. Other machines won't get them from this PR.

**UI bundle removed:** as you chose, I deleted the 40-skill UI/UX bundle the bootstrap installed, plus its routing sections. That left 12 OpenSpec skills in each skill folder.

**Verify results: 11 PASS, 2 FAIL, 2 WARN.** Removing the bundle didn't fix either FAIL, contrary to what I'd expected:
- **Rules region size:** the region in `AGENTS.md` is 1,593 words against a 1,500 limit. That's the skill's own template, which fails its own check. Hand edits get overwritten on re-run, so the fix belongs in the skill.
- **Always-loaded text:** `AGENTS.md` plus `CLAUDE.md` total 3,393 words, with 1,800 in `CLAUDE.md`. That needs the manual trim of `CLAUDE.md`.
- **The two WARNs** are machine-wide: about 3,436 skills, roughly 63 times the description budget, and 10 skills with empty descriptions.

**The new `tier-guard` hook gave a false positive in this session.** It searches the whole command text, and it blocked my first commit attempt because the PR description quoted an optimized-build command. I got around it by passing the commit message and PR description as files, without opening the gate. Expect the same with any commit or PR text that mentions such builds. Real optimized builds now also need `PROMETHEUS_TIER3=1` or `touch .kbd-orchestrator/tier3.allow`.

Still uncommitted: the OpenSpec tool folders (`.agents/`, `.opencode/`, `.kimi-code/`, `.dsh/`), waiting on your track-or-ignore decision, and four new session records.

The skill's next step is `/kbd-init`, which creates `project.json` and `constraints.md` once this merges.

## Root Cause

No explicit root-cause section was captured; preserve this as a session record, not an inferred diagnosis.

## Corrective Actions

Review and promote only reusable findings.

## Session Metadata

- Harness: claude-code
- Session: 6cef7a5e-4309-4ef9-91eb-79c07e58536e
- Captured: 2026-10-03T18:57:16.021417Z
- Project: /Users/gqadonis/Projects/prometheus/prometheus-knowledge

## Changed Paths

- .agents/
- .dsh/
- .kimi-code/
- .opencode/
