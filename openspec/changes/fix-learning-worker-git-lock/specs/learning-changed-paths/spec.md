## ADDED Requirements

### Requirement: Background status SHALL avoid optional index locks
The learning worker SHALL collect changed paths without taking Git's optional index-refresh lock and SHALL never remove an existing Git lock.

#### Scenario: Deadline terminates status
- **WHEN** real Git status remains running beyond the five-second collection deadline
- **THEN** the worker terminates and reaps its child, reports a timeout warning and leaves no new index.lock behind
- **AND** a subsequent Git write succeeds

#### Scenario: Another writer owns a lock
- **WHEN** a lock exists before collection
- **THEN** the lock and its contents remain untouched

### Requirement: Top-level changed-path reporting SHALL preserve its contract
The worker SHALL preserve top-level changed-path parsing, the 40-path cap and collector-output exclusions while omitting submodule changes.

#### Scenario: Normal repository changes
- **WHEN** tracked, staged, deleted, renamed or untracked top-level paths exist
- **THEN** the session packet reports those paths using the existing representation

#### Scenario: Nested or collector-only changes
- **WHEN** submodule worktrees and the collector's knowledge directory change
- **THEN** those changes are excluded from the session changed-path list
