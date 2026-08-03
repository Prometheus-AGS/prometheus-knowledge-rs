# prometheus-knowledge

Rust knowledge and learning runtime for human-readable Markdown records, immutable scoped prompt snapshots, atomic local enqueue, and durable Memory v2 receipt reconciliation.

## Runtime design

- **Project, shared, and global snapshots** publish as immutable generations with atomic `current` pointers.
- **Bounded prompt context** reads one validated generation per scope and applies deterministic size budgets.
- **Stop hooks** publish a private queue record by fsync plus atomic rename; they do no inference or network work.
- **`prometheus-learning-worker`** owns extraction, queue transitions, Memory operation submission, receipt reconciliation, and snapshot publication.
- **`pk doctor --json`** diagnoses the active plugin generation, stable dispatchers, snapshots, queue state, hook log permissions, and project scope without creating or changing state.

Queue states are explicit. Learning jobs use `pending → processing → completed | rejected`. Memory delivery uses `pending → submitting → accepted → completed | rejected`. Legacy retry/dead-letter directories are migration evidence and must be reconciled rather than treated as success.

Canonical documentation is published under [Knowledge & Learning](https://prometheus-ags.github.io/prometheus-skill-system/docs/knowledge-learning/snapshots-and-context).

## Binaries

- `pk` — ingest, lint, search, inspect, snapshot, migrate, and diagnose knowledge.
- `pk-cherry` — HTTP MCP bridge on the configured loopback address.
- `prometheus-learning-worker` — deterministic queue, receipt, and snapshot worker.

## Build and test

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p pk-cli -p pk-cherry -p prometheus-learning-worker
```

Typical read-only checks:

```bash
pk lint
pk doctor --json
```

`pk doctor` exits nonzero when required current-runtime evidence is absent or invalid. It does not open/create the knowledge store, repair queues, publish snapshots, or contact Memory.

## Recovery

On worker interruption, preserve every queue record. Reuse the stored operation ID and payload hash, reconcile the v2 receipt, move terminal evidence to `completed` or `rejected`, and publish a new snapshot generation only after durable completion.
