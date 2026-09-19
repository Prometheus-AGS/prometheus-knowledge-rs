## Purpose

Keep asynchronous learning progress bounded and recoverable when wiki aliases or an unavailable memory ledger would otherwise stall the durable queue.

## ADDED Requirements

### Requirement: Wiki source identity follows the logical article path
The store SHALL track each wiki article by its canonical wiki root and wiki-relative article path so distinct aliases remain independently reconcilable even when they resolve to the same filesystem target.

#### Scenario: One shared-target alias is removed
- **WHEN** two wiki-relative Markdown aliases resolve to one target and one alias is removed after the store opens
- **THEN** reconciliation removes only the missing alias entry and retains the other entry

### Requirement: Memory reconciliation is bounded and durable
The learning worker SHALL apply a finite timeout to memory-ledger requests, preserve an operation in durable storage when a request times out or cannot connect, and end the current memory reconciliation pass after that transport failure.

#### Scenario: Operation lookup never responds
- **WHEN** the memory ledger accepts an operation lookup but does not return a response within the configured request timeout
- **THEN** the worker records the error, retains the operation for a later reconciliation pass, releases the queue lock, and completes the run

### Requirement: Learning jobs progress independently of memory availability
The learning worker SHALL finish available learning jobs before reconciling memory operations so a memory-ledger outage does not prevent local learning artifacts from reaching their durable completed state.

#### Scenario: Jobs exist while the ledger is unresponsive
- **WHEN** learning jobs are pending and the memory ledger operation endpoint is unresponsive
- **THEN** the worker completes the local jobs, enqueues their memory operations durably, and exits after the bounded reconciliation attempt
