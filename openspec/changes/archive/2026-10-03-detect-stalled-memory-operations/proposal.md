## Why

The learning worker leaves a memory operation in `memory/accepted` for as long as
the server's receipt is non-terminal, and counts every successful re-poll as
delivered. When the server strands operations
(Prometheus-AGS/surreal-memory-server#30), the worker polls the same records on
every run, writes `memoryDelivered: 20, memoryAwaitingReconciliation: 0`, and
gives the operator no age, no stale signal, and no supported way to take the
records out of the loop (issue #16).

## What Changes

- Persist receipt bookkeeping on each operation: first acceptance, last receipt
  state and progress sequence, when the receipt last changed, consecutive
  unchanged polls, and the next poll time.
- Count only local state transitions as `memoryDelivered`; report re-polled
  operations as `memoryInFlight` and backed-off ones as `memoryDeferred`.
- Back off polling of unchanged accepted operations exponentially, 1m to 1h.
- Report `memoryStale`, `memoryStalled`, `oldestAcceptedAgeSeconds` and
  `staleAfterSeconds` in `status.json` and `status`. The threshold is set with
  `--stale-after` / `PROMETHEUS_LEARNING_STALE_AFTER`, default 6h.
- Add `quarantine [--older-than] [--dry-run]` (accepted → `memory/stalled`) and
  `release (--all | <id>...)` (stalled → accepted or pending), each writing a
  manifest to `memory/manifests/`.
- `pk doctor` reports the stalled count without failing on it.

## Impact

`pk-learning-worker` and the `pk doctor` detail string. The new record fields are
optional and omitted until set, so existing queue files parse unchanged.
`memoryDelivered` changes meaning: re-polling an accepted operation no longer
counts. No dependency, receipt protocol, or server change.
