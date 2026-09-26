# AGENTS.md

## Rust development

For Rust code, Cargo workspaces, manifests, compiler diagnostics, or Rust
architecture, load `prometheus-rust-workspace` first. It uses `rust-router`
to select the minimum relevant installed skills; its on-demand catalog includes
the language-mechanics, codebase-analysis, unsafe-Rust, and domain skills.

Always use `rust-best-practices` for general implementation and review. Add
`rust-async-patterns` for Tokio, concurrency, or cancellation work, and
`rust-mcp-server-generator` for Rust MCP server or transport work. Repository
dependency pins and protocol contracts override generator examples.

Skill activation does not authorize immediate Cargo execution. Finish a meaningful
set of production functionality, then run one serialized validation batch at the
completed change or phase boundary. Start with the smallest integration target that
exercises the real production path and collaborators. Unit, module-local, mock-only,
and filtered function tests do not count as completion evidence. Defer broader,
specialized, release, and feature-matrix checks until the applicable final boundary.
Read the skill's phase-gated verification reference before the first Cargo command.
