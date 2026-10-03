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

Each command writes its manifest before moving anything, so a partial run is
still on record. `quarantine` renames and then rewrites the state, and `release`
rewrites and then renames. Either way, a crash between the two steps never leaves
a `stalled` record in `memory/accepted`. `release` also records the receipt's
state and progress pair. Without it, a record quarantined before its first poll
would treat that poll as a first observation and reset its clock to the server's
old `updated_at`.

A record reconciled out of `memory/submitting` in a run is not polled again when
`memory/accepted` is listed later in the same run. Durations of zero are
rejected. Unreadable accepted records are skipped by `quarantine`, as they are by
`status`, so one bad file does not block quarantining the rest.

## Out of scope

The `docs/guide/13-tools-reference.md` update and the `learning.worker` doctor
check live in prometheus-skill-system (#118).
