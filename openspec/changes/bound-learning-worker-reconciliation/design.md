## Context

`run_once` already completes local jobs before memory reconciliation, but its HTTP client had no request timeout. The uncomfortable fact is that the ledger readiness endpoint can return healthy while an individual operation lookup hangs forever. Separately, `scan_wiki_tree` canonicalized every article even though reconciliation only needs a stable logical key; under 30 GB of observed swap use, one 576-file scan did not finish after seven minutes.

## Goals

- Bound a worker run when the ledger transport is unavailable.
- Preserve every queued operation and its identity across that failure.
- Remove per-article canonicalization while preserving root-symlink stability and independent alias identity.

## Non-Goals

- Change the durable queue schema or memory-ledger protocol.
- Retry or discard operations inside one failed pass.
- Change Markdown parsing or article identifiers.

## Decisions

### One request timeout and a pass-level transport circuit

Build the production HTTP client with a ten-second request timeout. On a reqwest timeout or connection failure, record the error for the current operation and stop the memory portion of that run. The next supervised run retries the durable operation. Protocol and receipt errors remain per-operation failures and do not stop unrelated reconciliation.

### Canonicalize the wiki root once

Resolve the wiki root once per scan and combine it with each path relative to that root. This preserves a stable key when the store root itself is a symlink and prevents two logical aliases to the same target from collapsing into one source-hash key.

### Persist recovery normalization only when it changes data

Restart recovery still normalizes legacy operations and persists any schema, payload, hash, or state change. When a submitting operation already equals that normalized form, recovery keeps the existing file rather than replacing and synchronizing identical bytes.

## Risks

A healthy but unusually slow ledger request can cross the ten-second ceiling and be retried on a later run. Operation IDs and the authoritative lookup-before-submit protocol keep that retry idempotent.

## Verification

- A delayed ledger fixture must time out promptly and leave the submitting file in place.
- A normalized submitting operation must retain its inode through restart recovery.
- Removing one of two Markdown aliases to a shared target must remove only that fallback-ID entry.
- Existing store and worker integration suites must remain green.
