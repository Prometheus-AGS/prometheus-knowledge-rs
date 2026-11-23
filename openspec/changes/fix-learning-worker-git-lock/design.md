# Design

## Context
The existing worker pipes status stdout, waits up to five seconds, then kills/reaps the child. Git's optional index refresh can leave index.lock behind on SIGKILL. The issue includes a process/lock trace and timing evidence.

## Decisions
Use git --no-optional-locks status --short --ignore-submodules=all. Keep the five-second deadline, parsing, 40-path bound, collector-output exclusions and failure fallback. Add a warning only for the timeout case. Never delete Git locks. No refactor, schema or version bump.

## Verification boundary
The approved useful outcome includes the complete worker change, exact downstream pins and local PK replacement. Production changes precede integration execution; repository VLBV overrides legacy team RED/GREEN instructions. No speculative compiler probes or broad workspace tests. Run the focused worker integration packet once after production completion; fix observed failures and rerun only affected entries under the user's implementation/fix authorization.

Use a real worker, isolated home/queue, real Git repositories and a large status output that blocks the existing undrained pipe beyond five seconds. Compare the original installed binary as the negative control. Assert no remaining index.lock and successful subsequent git add; confirm timeout evidence. Exercise top-level paths, submodule exclusions and a pre-existing lock sentinel. Run worker and attribution integration targets.

## Rollout
Push PK first; pin both consumers to its full SHA. Build locked release pk, pk-cherry and prometheus-learning-worker. Back up executable/launch-agent state and quiesced queues. Stop only PK services, atomically replace/sign binaries, reload existing service definitions without changing data roots, verify isolated installed behavior and health. Retain backups and provenance.

## Constraints
No platform claims beyond locally executed macOS verification. Existing user work and other branches remain intact. No tags, PR merges or unrelated service restarts.
