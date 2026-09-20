## Why

The deployed memory ledger now performs deterministic receipt lookup and
bounded startup validation, but a cold valid lookup measured 19.64 seconds on
the release host. The worker's fixed ten-second request bound caused two
consecutive supervised runs to stop on the first durable receipt with no
progress.

## What Changes

- Set the production memory-ledger request budget to 30 seconds.
- Keep the existing pass-level stop on timeout or connection failure.
- Preserve every durable operation for the next supervised pass.

## Impact

An unavailable ledger can now hold each request for at most 30 seconds instead
of ten. A reconciliation can issue lookup, readiness, and submission requests,
and each has its own bound. No queue schema, retry count, or receipt protocol
changes.
