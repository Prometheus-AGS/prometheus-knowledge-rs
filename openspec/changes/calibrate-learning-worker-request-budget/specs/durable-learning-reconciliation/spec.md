## MODIFIED Requirements

### Requirement: Memory reconciliation is bounded and durable

The learning worker SHALL apply a 30-second timeout to each production
memory-ledger request, preserve an operation in durable storage when a request
times out or cannot connect, and end the current memory reconciliation pass
after that transport failure.

#### Scenario: Cold valid receipt lookup

- **WHEN** a valid durable receipt lookup takes more than ten seconds but less
  than 30 seconds on the release host
- **THEN** the worker accepts the authoritative receipt and advances the queue

#### Scenario: Operation lookup exceeds the production budget

- **WHEN** the memory ledger does not return within 30 seconds
- **THEN** the worker records the error, retains the operation for a later pass,
  releases the queue lock, and completes the run
