## Change detection

A receipt has changed when its `(state, progress_seq)` differs from the pair
recorded on the operation. `updated_at` is not compared, because a server may
touch it without making progress. A change resets `unchangedPolls` and
`nextPollAt` and sets `lastReceiptChangeAt` to now.

## Records written by an older worker

A record with no bookkeeping takes `lastReceiptChangeAt` from the receipt's
`updated_at` and `firstAcceptedAt` from its `created_at`, each clamped to now.
Starting the clock at now instead would give an operation stranded for twelve
days a fresh six-hour window on upgrade, which hides exactly the condition this
change exists to report.

## Staleness

`stale = now - (lastReceiptChangeAt ∥ firstAcceptedAt ∥ queuedAt) > threshold`.
It is computed over `memory/accepted` after reconciliation, independent of
whether a record was polled this run, so a backed-off record is still reported.

## Backoff

After `n` consecutive unchanged polls, the next poll waits `min(60s · 2^(n-1), 1h)`.
Only `memory/accepted` is deferred; `pending` and `submitting` always proceed,
because their outcome is not yet known. The cap keeps a stranded operation that
the server later commits from waiting more than an hour.

## Quarantine and release

`memory/stalled` is separate from `memory/rejected` so that intent stays clear
and records stay retryable. Both commands take `worker.lock` and fail if a worker
holds it. A dry run reads only: it neither takes nor creates the lock and writes
no manifest. Every target is checked before anything moves, so a conflict or an
unknown id leaves the queue unchanged. `release` restarts the staleness clock but
keeps `firstAcceptedAt`, so the oldest-age metric still reflects the real wait.

## Out of scope

The `docs/guide/13-tools-reference.md` update and the `learning.worker` doctor
check live in prometheus-skill-system (#118).
