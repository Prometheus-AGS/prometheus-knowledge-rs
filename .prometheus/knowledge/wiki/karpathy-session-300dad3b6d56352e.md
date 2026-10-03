---
type: SessionRecord
id: karpathy-session-300dad3b6d56352e
title: Karpathy session 300dad3b6d56
tags:
- karpathy
- session-learning
sources:
- session:6cef7a5e-4309-4ef9-91eb-79c07e58536e
timestamp: 2026-10-03T19:09:28.044228+00:00
created_at: 2026-10-03T19:09:28.044228+00:00
updated_at: 2026-10-03T19:09:28.044228+00:00
revision: 0
---

## Delta

Completed agent-team-creator — prometheus-knowledge-maintainers (5 roles, 6 native targets)

I created and installed a 5-role team, `prometheus-knowledge-maintainers`, for Claude Code, Codex, Kimi Code, OpenCode and DeepSeek Harness. MiniMax is staged but not installed. Nothing is committed yet; everything is on the branch `chore/agent-team-knowledge-maintainers`.

**How I got to these roles.** One agent mapped this repo's crates and their links to the other repositories. A separate critic then reviewed my 7-role draft, seeing only that research and the draft. It found real defects:
- several files had no owner, including `CLAUDE.md`, `openspec/specs/` and `.claude/`;
- `Cargo.lock` couldn't have one owner, since every dependency change rewrites it;
- splitting the core crates across engineers would leave the build broken between handoffs;
- the reviewer wasn't actually independent on Kimi, DeepSeek or MiniMax;
- nothing protected the contracts that other repos depend on.

I adopted its corrected 5-role design.

| Role | Responsibility | Owns |
|---|---|---|
| `knowledge-lead` | Runs OpenSpec and KBD, routes work, approves dependency requests, writes notes for other repos. It's the only writer of shared files and never certifies its own work. | `openspec/`, `.kbd-orchestrator/`, `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.github/`, `README.md`, `CLAUDE.md`, `AGENTS.md`, `.claude/`, `docs/`, `scripts/` |
| `core-engineer` | Domain types, the store and OKF format, the watcher, librarian and event store. A `pk-core` change and its knock-on fixes land as one commit set. | `pk-core`, `pk-store`, `pk-watcher`, `pk-librarian`, `pk-event-store` |
| `surfaces-engineer` | The MCP server, Cherry bridge, UAR adapter, `pk` CLI and deployment. | `pk-mcp`, `pk-cherry`, `pk-uar`, `pk-cli`, `deployment/` |
| `learning-worker-engineer` | The learning queue, `status.json`, and the client for surreal-memory-server's ledger. | `pk-learning-worker` |
| `verifier` | Adversarial review plus the single integration gate (format, lint, tests), run in its own build folder. It never edits code. | `.agent-team/…/findings/` |

Every role's instructions carry the same standing rules:
- **Frozen contracts:** the MCP tools, port 8942, `pk context` output, `pk ingest` arguments, the queue layout, `status.json` fields and OKF v0.2 can't change without an OpenSpec change that names the affected repos.
- **Semver:** no release tag while a breaking API change is unresolved. That currently blocks tagging 1.9.0 because of the `WikiEntry.sources` change.
- **Other repos:** never edit them; write a handoff note instead.
- **Ledger:** surreal-memory-server's memory-core team owns the ledger API.
- **Off-limits files:** never edit `versions.toml` or `.prometheus/`.
- **Independence:** review done in the builder's own context must be labelled as not independent.

**What's installed and what's only staged:**
- **Installed** (agent files in `.claude/agents/`, `.codex/agents/`, `.kimi-code/agents/`, `.opencode/agents/` and `.dsh/profiles/`):
  - Each was written to that harness's published format but not tested live in it.
  - The DeepSeek team support is experimental and gives every member the same model.
  - Kimi also ignores the per-role model setting.
- **Routing:** a managed routing block in `AGENTS.md` and `CLAUDE.md` makes this team the default for all code work. The installer's `--check` passes.
- **MiniMax (staged only):** its agents live in the global `~/.minimax/agents/`, which this project installer never writes. To use them, copy them yourself:

```bash
cp -R /private/tmp/claude-501/-Users-gqadonis-Projects-prometheus-prometheus-knowledge/6cef7a5e-4309-4ef9-91eb-79c07e58536e/scratchpad/export-minimax/agents/. ~/.minimax/agents/
```

  Even then, `mcode exec` can't select a custom agent, so they're only usable interactively.
- **Not set: models.** Roles carry tier hints (hard or medium), not actual model names. Choosing concrete models per harness is a separate st

## Root Cause

No explicit root-cause section was captured; preserve this as a session record, not an inferred diagnosis.

## Corrective Actions

Review and promote only reusable findings.

## Session Metadata

- Harness: claude-code
- Session: 6cef7a5e-4309-4ef9-91eb-79c07e58536e
- Captured: 2026-10-03T19:09:27.687571Z
- Project: /Users/gqadonis/Projects/prometheus/prometheus-knowledge

## Changed Paths

- AGENTS.md
- CLAUDE.md
- .agent-team/
- .agents/
- .codex/
- .dsh/
- .kimi-code/
- .opencode/
