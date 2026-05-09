# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Documentation hierarchy

This file covers **prometheus-knowledge-specific** rules only:
Rust workspace conventions, crate architecture, model routing, and
librarian-specific patterns.

For project-wide conventions (KBD lifecycle, skill discovery, OpenSpec,
progress signaling, memory workflow, BDD rules), see:

> **Canonical rules:** `prometheus-skill-pack/CLAUDE.md`
> Path: `/Users/gqadonis/Projects/prometheus/prometheus-skill-pack/CLAUDE.md`

When a rule here conflicts with the canonical file, the canonical file wins.
Add project-wide rules only to the canonical file — not here.

## Overview

prometheus-knowledge is the Karpathy LLM Knowledge Base method implemented in Rust — a self-maintaining, human-readable Markdown wiki compiled and linted by LLMs. The system bypasses vector databases and black-box embeddings by using flat markdown files with TF-IDF search, where every fact traces to a readable `.md` file.

**Core Principle**: Treat raw sources as immutable, compile them into structured wiki articles using LLMs, and keep all knowledge in git-trackable markdown files with YAML frontmatter.

## Architecture

This is a multi-crate Rust workspace following domain-driven design:

```
pk-core        — Domain types: WikiEntry, RawDoc, LintReport, LibrarianEvent
pk-store       — Flat-file MarkdownStore + in-memory TF-IDF index
pk-watcher     — notify-rs FSEvents/inotify → tokio inbox channel
pk-librarian   — Librarian: compile/lint/focus/auto_fix + ModelRouter
pk-mcp         — Axum SSE server: POST /mcp (JSON-RPC) + GET /events (SSE)
pk-uar         — UAR integration (registry + in-process runner + MCP client)
pk-cherry      — Binary: Cherry Studio MCP bridge
pk-cli         — Binary: CLI for all operations
```

**Key Flow**:
1. Raw docs dropped into `raw/` (watched by `pk-watcher`)
2. `Librarian` compiles them via LLM into structured `WikiEntry` objects
3. Entries saved as markdown files with YAML frontmatter in `wiki/`
4. TF-IDF index updated in-memory for fast search
5. Periodic lint passes detect contradictions and gaps

## Development Commands

### Build

```bash
# Build both binaries
cargo build --release -p pk-cherry -p pk-cli

# Build all workspace crates
cargo build --workspace

# Build specific crate
cargo build -p pk-core
```

### Testing

```bash
# Run all tests
cargo test --workspace

# Run tests for specific crate
cargo test -p pk-store

# Run single test
cargo test -p pk-core -- types_tests::test_article_id_from_slug

# Run with output
cargo test -- --nocapture
```

### Code Quality

```bash
# Format (always before committing)
cargo fmt --all

# Lint
cargo clippy --workspace -- -D warnings

# Check compilation (faster than build)
cargo check --workspace

# Coverage
cargo llvm-cov --html
```

### Running the System

```bash
# Set up environment (copy .env.example → .env and configure)
cp .env.example .env

# Start Cherry Studio MCP bridge
PK_KB_DIR=~/.prometheus/knowledge ./target/release/pk-cherry

# CLI operations
./target/release/pk ingest session-notes.md --source "session:uar-2026-04-10"
./target/release/pk lint
./target/release/pk lint --fix
./target/release/pk focus "UAR session bus architecture"
./target/release/pk search "axum tower middleware"
./target/release/pk list
```

## Key Design Patterns

### Model Routing Strategy

The `ModelRouter` routes different task types to appropriate models:
- **Compile** (high quality): Cloud model (Claude Sonnet 4.6) via liter-llm gateway
- **Lint/Focus/Fix** (cheap, fast): Local Qwen 2.5 via Cherry Studio or mistral.rs

This two-tier approach balances quality and cost:
- Compilation requires deep understanding → expensive model
- Linting/focus operations are iterative → cheap local model

Configure via environment variables:
```bash
PK_COMPILE_MODEL=claude-sonnet-4-6
PK_LINT_MODEL=qwen2.5-14b-instruct-q4_k_m
CLOUD_LLM_URL=https://api.anthropic.com/v1
LOCAL_LLM_URL=http://localhost:1234/v1
```

### Domain Types (pk-core)

**WikiEntry**: Compiled knowledge article with YAML frontmatter
- `id: ArticleId` — slugified from title, zero-cost newtype
- `content: String` — markdown body
- `tags`, `links`, `sources` — structured metadata for search and linking
- `revision: u32` — incremented on each upsert

**RawDoc**: Unprocessed document from `raw/` inbox
- Immutable source material (PDFs, markdown, text, JSON)
- `session_id` — optional trace to originating agent session

**LintReport**: Issue found during lint pass
- `severity: Info | Warning | Error`
- `auto_fixable: bool` — whether `auto_fix()` can repair it

**LibrarianEvent**: Broadcast events for UAR integration
- Maps to AG-UI `ThinkingMessage` events
- Emitted on compile, lint, focus, update, error

### Store Pattern (pk-store)

**MarkdownStore**: Flat-file persistence with in-memory index
- Each `WikiEntry` → single `.md` file with YAML frontmatter
- In-memory `HashMap<ArticleId, WikiEntry>` for fast lookups
- `TextIndex` (TF-IDF) for keyword search
- `RwLock` for concurrent read/write access

Key operations:
- `upsert()`: Write entry to filesystem + update index
- `search(query, k)`: TF-IDF ranking, returns top k entries
- `related_entries()`: Find entries similar to a raw doc (used during compilation)

### Librarian Workflow (pk-librarian)

The `Librarian` orchestrates LLM-driven operations:

**Compile**: `RawDoc → WikiEntry`
1. Search for related existing entries
2. Call compile model with context
3. Parse JSON response into `WikiEntry`
4. Upsert to store
5. Broadcast `LibrarianEvent::Compiled`

**Lint**: Scan all entries for issues
1. Snapshot entire wiki
2. Serialize as JSON
3. Call lint model
4. Parse JSON array of `LintReport`
5. Broadcast `LibrarianEvent::LintCompleted`

**Focus**: Build mini-KB for a topic
1. Search for top k matching entries
2. Call focus model with candidates
3. Return synthesized markdown brief

**Auto-fix**: Apply suggested fixes
1. Load entry by ID from lint report
2. Call fix model with issue + suggestion
3. Update entry content
4. Upsert and broadcast update event

### Event Broadcasting (LibrarianEvent)

The Librarian uses `tokio::sync::broadcast` to emit events for:
- UAR agent integration (forward to AG-UI SSE stream)
- Real-time progress monitoring
- Audit logging

Event types:
- `Compiled { entry_id, title, tags }`
- `LintCompleted { reports, entry_count }`
- `Focused { topic, entry_count }`
- `Updated { entry_id, revision }`
- `Error { message }`

### Immutability and Revision Tracking

Following Rust conventions:
- `WikiEntry` fields are owned values (not references)
- New entries created via builder pattern: `WikiEntry::new().with_tags().with_sources()`
- Updates create new objects (not mutations)
- `bump_revision()` updates `updated_at` timestamp and increments counter

## File Organization

```
~/.prometheus/knowledge/
├── raw/                     ← inbox (watched by pk-watcher)
│   └── *.md / *.txt         ← drop files here
└── wiki/                    ← compiled articles
    ├── universal-agent-runtime.md
    ├── mempalace-rs.md
    └── ...
```

Each wiki article structure:
```markdown
---
id: universal-agent-runtime
title: Universal Agent Runtime
tags: [rust, uar, prometheus]
links: [prometheus-mesh-iroh, liter-llm-provider]
sources: [session:uar-2026-04-10]
created_at: 2026-04-10T12:00:00Z
updated_at: 2026-04-10T12:00:00Z
revision: 3
---

Article content in markdown...
```

## Integration Points

### Cherry Studio MCP Server

Add to Cherry Studio's MCP config:
```json
{
  "name": "prometheus-knowledge",
  "url": "http://localhost:8942/mcp",
  "transport": "sse"
}
```

Available MCP tools:
- `knowledge_ingest` — Compile raw content
- `knowledge_lint` — Scan for issues
- `knowledge_focus` — Build mini-KB
- `knowledge_search` — TF-IDF search
- `knowledge_get` — Retrieve article

### UAR Integration

Wire the Librarian's event bus into UAR session bus:
```rust
let event_rx = librarian.event_tx.subscribe();
// Forward to AG-UI SSE stream as ThinkingMessage events
```

The MCP tools are exposed via `/mcp` endpoint — add to liter-llm tool registry.

### Claude Code Integration

Use the provided slash commands:
- `/focus <topic>` — Load focused context from knowledge base
- `/ingest [file]` — Save session notes to wiki

Add post-session hook for automatic ingestion:
```bash
echo "$SESSION_SUMMARY" | pk ingest --source "session:$(date +%s)"
```

## Testing Strategy

Follow TDD workflow per `common/testing.md`:
1. Write test first (RED)
2. Run test — should fail
3. Implement minimal code (GREEN)
4. Run test — should pass
5. Refactor (IMPROVE)
6. Verify coverage ≥ 80%

Test organization:
- Unit tests in `#[cfg(test)]` modules within each crate
- Integration tests in `tests/` directories
- Use `rstest` for parameterized tests
- Use `mockall` for trait mocking

## Common Patterns

### Adding a New Domain Type

1. Define in `pk-core/src/types.rs` with `Serialize + Deserialize`
2. Add newtype wrapper if it needs type safety (like `ArticleId`)
3. Update related structs (`WikiEntry`, `RawDoc`, etc.)
4. Write unit tests in same file
5. Update store operations in `pk-store` if needed

### Adding a New Librarian Operation

1. Define system prompt in `pk-librarian/src/prompts.rs`
2. Add user prompt builder function
3. Add method to `Librarian` struct
4. Route to appropriate model via `TaskKind` enum
5. Parse LLM response and handle errors
6. Emit `LibrarianEvent` for observability
7. Add CLI command in `pk-cli/src/main.rs`

### Adding a New MCP Tool

1. Define tool handler in `pk-mcp/src/tools.rs`
2. Add to MCP tool registry in `pk-mcp/src/server.rs`
3. Wire to Librarian method
4. Test via Cherry Studio or MCP client
5. Update README with tool documentation

## Critical Constraints

1. **No Vector Databases**: This is a deliberate architectural choice. Use TF-IDF search only.
2. **Flat File Storage**: All wiki entries must be readable markdown files — no binary formats.
3. **Git-Trackable**: Every change should be diffable and forkable.
4. **Model Routing**: Always use cheap models for lint/focus/fix to control costs.
5. **Immutability**: Follow Rust conventions — create new values instead of mutating.
6. **Error Handling**: Use `anyhow::Result` in applications, `thiserror` for library errors.
7. **No unwrap()**: Always handle errors explicitly with `?` or proper error context.

## When Working on This Codebase

1. **Read the Karpathy research** to understand the pattern philosophy (anti-vector-DB, pro-markdown)
2. **Check environment variables** via `.env.example` before running services
3. **Use model routing appropriately** — don't call expensive models for cheap operations
4. **Test with both cloud and local models** to verify routing works
5. **Verify markdown output** after changes to ensure frontmatter stays valid
6. **Run lint passes** after adding new wiki entries to catch contradictions
7. **Keep event broadcasting** for UAR integration observability

## Related Projects

- **mempalace-rs**: Hot episodic agent memory (ChromaDB-equivalent vectors) — complements this cold storage
- **liter-llm**: Model gateway for unified API access
- **UAR (Universal Agent Runtime)**: Execution substrate that consumes `LibrarianEvent` stream
- **Cherry Studio**: Local LLM server with MCP support

## Slash Command Prefix Convention (SP-017)

prometheus-knowledge slash commands use the `pk-` prefix to avoid collisions with commands from `prometheus-skill-pack` or other installed packs.

**Rule:** All `.claude/commands/*.md` files in this repo must use the `pk-` prefix.

| Current name | Canonical name | Notes |
|---|---|---|
| `focus.md` | `pk-focus.md` | skill-pack owns `/focus` as the higher-level user-facing command |
| `ingest.md` | `pk-ingest.md` | pk-specific librarian operation |

**Why:** Claude Code load order for slash commands is non-deterministic when multiple packs define the same name. The prefix guarantees unambiguous dispatch.

To detect conflicts across installed packs:
```bash
bash prometheus-skill-pack/scripts/detect-command-conflicts.sh
```
