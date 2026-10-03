## ADDED Requirements

### Requirement: Delivery counts reflect progress

The learning worker SHALL count a memory operation as delivered only when a
receipt moves it to a new local state, and SHALL report polled operations whose
receipt is still non-terminal as in flight.

#### Scenario: Re-polling an accepted operation

- **WHEN** an operation already in `memory/accepted` is polled and its receipt is
  still non-terminal
- **THEN** `status.json` reports it in `memoryInFlight` and not in
  `memoryDelivered`

### Requirement: Stranded operations are reported stale

The learning worker SHALL record when each accepted operation's receipt state or
progress sequence last changed, and SHALL report as stale every accepted
operation without such a change for longer than the configured threshold
(default 6h), together with the age of the oldest accepted operation.

#### Scenario: Server keeps answering planned

- **WHEN** the server's receipt for an accepted operation has not changed for
  longer than the threshold
- **THEN** `status.json` and `status` report `memoryStale = 1` and
  `memoryDelivered = 0`

#### Scenario: Record written by an older worker

- **WHEN** an accepted operation has no bookkeeping and its receipt was last
  updated twelve days ago
- **THEN** it is reported stale on its first poll

### Requirement: Unchanged receipts back off

The learning worker SHALL delay the next poll of an accepted operation whose
receipt did not change, doubling the delay from one minute up to one hour, and
SHALL reset the delay when the receipt changes.

#### Scenario: Run inside the backoff window

- **WHEN** a run starts before an unchanged operation's next poll time
- **THEN** the worker does not contact the server for it and counts it in
  `memoryDeferred`

### Requirement: Operators can quarantine and release stalled operations

The learning worker SHALL provide `quarantine`, which moves stale accepted
operations to `memory/stalled` and writes a manifest, and `release`, which
returns stalled operations to `memory/accepted` (or `memory/pending` when there
is no receipt) and restarts their staleness clock.

#### Scenario: Dry run

- **WHEN** `quarantine --dry-run` runs
- **THEN** it lists exactly the stale operations and changes no file

#### Scenario: Round trip

- **WHEN** `quarantine` is followed by `release --all`
- **THEN** each operation is back in `memory/accepted` with the same id and
  payload hash, and both manifests exist

#### Scenario: Unknown id

- **WHEN** `release` names an operation that is not in `memory/stalled`
- **THEN** the command fails and moves nothing
