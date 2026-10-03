# prometheus-knowledge

Rust knowledge and learning runtime for human-readable Markdown records, immutable scoped prompt snapshots, atomic local enqueue, and durable Memory v2 receipt reconciliation.

## Runtime design

- **Project, shared, and global snapshots** publish as immutable generations with atomic `current` pointers.
- **Bounded prompt context** reads one validated generation per scope and applies deterministic size budgets.
- **Stop hooks** publish a private queue record by fsync plus atomic rename; they do no inference or network work.
- **`prometheus-learning-worker`** owns extraction, queue transitions, Memory operation submission, receipt reconciliation, and snapshot publication.
- **`pk doctor --json`** diagnoses the active plugin generation, stable dispatchers, snapshots, queue state, hook log permissions, and project scope without creating or changing state.

**Wiki index and log writes** rebuild `index.md` from the pages on disk under an exclusive `wiki/.index.lock`, keep entries for pages the running binary cannot parse, and replace `index.md` and `log.md` atomically. Projects that commit their wiki should ignore `wiki/.index.lock` and `wiki/.*.tmp`. Keep `pk`, `pk-cherry` and `prometheus-learning-worker` at the same version: an older writer still rebuilds the index from only the pages it can read.

Queue states are explicit. Learning jobs use `pending → processing → completed | rejected`. Memory delivery uses `pending → submitting → accepted → completed | rejected`. Legacy retry/dead-letter directories are migration evidence and must be reconciled rather than treated as success.

### Stalled memory operations

An operation stays in `memory/accepted` while the server's receipt is non-terminal (`accepted`, `validated`, `blocked`, `planned`, `processing`, `indexed`). The worker records `firstAcceptedAt`, `lastReceiptState`, `lastReceiptProgressSeq` and `lastReceiptChangeAt` on each record. When a poll returns an unchanged receipt, it backs off before polling again: 1m, 2m, 4m and so on, capped at 1h (`unchangedPolls`, `nextPollAt`). An accepted operation is **stale** when its receipt has not changed for longer than `--stale-after` (env `PROMETHEUS_LEARNING_STALE_AFTER`, default `6h`; units `s|m|h|d`).

`status.json`, written by every `run-once`:

| Field | Meaning |
|---|---|
| `memoryDelivered` | Operations that moved to a new local state this run (accepted, completed, rejected). Re-polling an accepted operation does not count. |
| `memoryInFlight` | Operations polled this run whose receipt is still non-terminal. |
| `memoryDeferred` | Accepted operations skipped this run because they are backing off. |
| `memoryAwaitingReconciliation` | Operations whose reconciliation failed this run (transport, contract, or server error). |
| `memoryStale` | Accepted operations with no receipt progress for longer than the threshold. |
| `memoryStalled` | Operations quarantined in `memory/stalled`. |
| `oldestAcceptedAgeSeconds` | Age of the oldest accepted operation, measured from first acceptance (`null` if none). |
| `staleAfterSeconds` | The staleness threshold in effect. |

`prometheus-learning-worker status [--json]` reports the same stale, stalled and oldest-age values next to the per-directory counts.

To take stale operations out of the redelivery loop, then return them once the server is fixed:

```bash
prometheus-learning-worker quarantine --dry-run          # list stale accepted operations; changes nothing
prometheus-learning-worker quarantine [--older-than 6h]  # move them to memory/stalled
prometheus-learning-worker release --all                 # or: release <operation-id>...
```

`release` returns an operation to `memory/accepted` if it already has a receipt, otherwise to `memory/pending`, and restarts its staleness clock. Both commands write a manifest to `memory/manifests/` and refuse to run while a worker holds the queue lock. `memory/stalled` is an operator decision, not an unsettled record: `pk doctor` reports its size but does not fail on it.

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
