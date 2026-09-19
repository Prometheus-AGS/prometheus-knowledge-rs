## Why

The asynchronous learning worker can hold its queue lock indefinitely when a memory-ledger operation lookup accepts a connection but never responds. Large wiki stores also perform one blocking canonicalization per article, which made a 576-file project take more than seven minutes to begin processing under observed swap pressure.

## What Changes

- Bound every memory-ledger request and end the current reconciliation pass after a transport timeout or connection failure while preserving queued operations.
- Identify wiki scan sources by one canonical store root plus each article's relative path, avoiding per-article canonicalization and keeping aliases independently reconcilable.
- Add regressions for an unresponsive ledger and two article aliases that share one target.

## Capabilities

### New Capabilities

- `durable-learning-reconciliation`: Durable, bounded reconciliation of learning jobs, wiki sources, and memory-ledger operations.

### Modified Capabilities

None.

## Impact

This changes `pk-learning-worker` request handling and `pk-store` source identity. It adds no dependency or public API and retains the existing durable queue formats.
