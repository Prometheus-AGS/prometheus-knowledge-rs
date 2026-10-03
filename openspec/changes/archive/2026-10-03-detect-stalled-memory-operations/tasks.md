## 1. Detection

- [x] 1.1 RED: binary test where the ledger keeps answering `planned` reports `memoryStale = 1`, `memoryDelivered = 0`, and is not re-polled inside the backoff window
- [x] 1.2 Persist receipt bookkeeping and seed it from receipt timestamps for legacy records
- [x] 1.3 Count only transitions as delivered; add in-flight, deferred, stale, stalled, and oldest-age to `status.json` and `status`
- [x] 1.4 Exponential poll backoff for unchanged accepted operations

## 2. Operator commands

- [x] 2.1 RED: `quarantine --dry-run` changes nothing; `quarantine` then `release` is a round trip; unknown ids fail without moving anything
- [x] 2.2 `quarantine` and `release` with manifests under `memory/manifests/`
- [x] 2.3 `pk doctor` reports the stalled count without failing on it

## 3. Documentation

- [x] 3.1 README: `status.json` fields, `--stale-after`, `quarantine`/`release`
