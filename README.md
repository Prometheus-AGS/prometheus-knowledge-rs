# prometheus-knowledge

The Karpathy LLM Knowledge Base method implemented in Rust — a self-maintaining,
human-readable Markdown wiki compiled and linted by LLMs. No vector database.
No black-box embeddings. Every fact traces to a readable `.md` file.

## Architecture

```
raw/           ← drop files here (or `pk ingest`)
wiki/          ← compiled articles (YAML frontmatter + markdown body)

pk-core        domain types: WikiEntry, RawDoc, LintReport, LibrarianEvent
pk-store       flat-file MarkdownStore + in-memory TF-IDF index
pk-watcher     notify-rs FSEvents/inotify → tokio inbox channel
pk-librarian   Librarian: compile / lint / focus / auto_fix + ModelRouter
pk-mcp         Axum SSE server: POST /mcp (JSON-RPC) + GET /events (SSE)
pk-cherry      binary: pk-cherry — Cherry Studio MCP bridge
pk-cli         binary: pk — CLI for all operations
```

## Quick Start

```bash
# Build both binaries
cargo build --release -p pk-cherry -p pk-cli

# Start the Cherry Studio bridge (default port 8942)
PK_KB_DIR=~/.prometheus/knowledge ./target/release/pk-cherry

# In another terminal — ingest a file
./target/release/pk ingest session-notes.md --source "session:uar-2026-04-10"

# Or pipe from stdin (e.g., after a Claude Code session)
cat session-summary.txt | ./target/release/pk ingest --source "pmpo:reflect"

# Lint the full knowledge base
./target/release/pk lint

# Auto-fix all fixable issues
./target/release/pk lint --fix

# Build a focused mini-KB for a topic (prints to stdout)
./target/release/pk focus "prometheus UAR session bus architecture"

# Load that into a Claude Code session
./target/release/pk focus "mempalace-rs vector storage" > /tmp/context.md
# Then in Claude Code: /context /tmp/context.md

# Search
./target/release/pk search "axum tower middleware"

# List all entries
./target/release/pk list
```

## Cherry Studio Integration

Add to Cherry Studio's MCP config (`Settings → MCP`):

```json
{
  "name": "prometheus-knowledge",
  "url": "http://localhost:8942/mcp",
  "transport": "sse"
}
```

Available MCP tools in every Cherry Studio chat:
| Tool | Description |
|---|---|
| `knowledge_ingest` | Compile raw content into the wiki |
| `knowledge_lint` | Scan for gaps and contradictions |
| `knowledge_focus` | Build a mini-KB for a topic |
| `knowledge_search` | TF-IDF search |
| `knowledge_get` | Retrieve a single article |

## UAR Integration

The Librarian's broadcast bus speaks `LibrarianEvent` which maps cleanly to
AG-UI `ThinkingMessage` events. Wire it into your existing session bus:

```rust
// In your UAR agent handler
let event_rx = librarian.event_tx.subscribe();
// Forward to AG-UI SSE stream as ThinkingMessage events
```

The MCP tools are exposed via the same `/mcp` endpoint — add `pk-cherry`'s
address to your UAR's liter-llm tool registry.

## Model Routing

Configure via environment variables:

```bash
# Compile (high quality — LLM structures the article)
export PK_COMPILE_MODEL=claude-sonnet-4-6
export CLOUD_LLM_URL=https://api.anthropic.com/v1
export CLOUD_LLM_API_KEY=sk-ant-...

# Lint + Focus (cheap + fast — local Qwen via Cherry Studio / mistral.rs)
export PK_LINT_MODEL=qwen2.5-14b-instruct-q4_k_m
export PK_FOCUS_MODEL=qwen2.5-14b-instruct-q4_k_m
export LOCAL_LLM_URL=http://localhost:1234/v1
```

## PMPO Reflect Hook

Add to your Claude Code `post_tool_use` hook to auto-ingest every session:

```bash
#!/bin/bash
# .claude/hooks/post-session.sh
SESSION_SUMMARY="$1"
SOURCE="pmpo:reflect:$(date +%Y-%m-%d)"
echo "$SESSION_SUMMARY" | pk ingest --source "$SOURCE"
```

Or from a Claude Code slash command:
```
/run echo "$SESSION_NOTES" | pk ingest --source "session:$(date +%s)"
```

## KB Directory Layout

```
~/.prometheus/knowledge/
├── raw/                     ← inbox (watched by pk-watcher)
│   └── *.md / *.txt         ← drop files here
└── wiki/                    ← compiled articles
    ├── universal-agent-runtime.md
    ├── mempalace-rs.md
    ├── prometheus-mesh-iroh.md
    └── ...
```

Each wiki article is a plain `.md` file with YAML frontmatter:

```markdown
---
id: universal-agent-runtime
title: Universal Agent Runtime
tags: [rust, uar, prometheus, liter-llm]
links: [prometheus-mesh-iroh, liter-llm-provider]
sources: [session:uar-2026-04-10]
created_at: 2026-04-10T12:00:00Z
updated_at: 2026-04-10T12:00:00Z
revision: 3
---

The UAR is the core execution substrate for Prometheus agents...
```

All files are plain text — git-trackable, diffable, forkable.

## Relationship to mempalace-rs

| | mempalace-rs | prometheus-knowledge |
|---|---|---|
| Storage | ChromaDB-equivalent vectors | Flat Markdown files |
| Primary use | Hot episodic agent memory | Cold durable engineering wiki |
| Retrieval | Semantic vector similarity | TF-IDF text search |
| Human readable | No (embeddings) | Yes (markdown) |
| Self-healing | No | Yes (lint passes) |
| Git-friendly | No | Yes |

Use both: mempalace-rs for working agent memory during a session,
prometheus-knowledge for the persistent wiki of architectural knowledge.
