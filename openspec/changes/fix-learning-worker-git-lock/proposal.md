# Proposal

## Why
PK issue #40 records repeated orphaned Git index locks after the learning worker kills a status process at its five-second deadline. Background session collection must not block subsequent repository writes.

## What Changes
- Disable optional Git locks and omit submodule inspection during changed-path collection.
- Warn on status timeout while retaining bounded collection and queue durability.
- Update both skill packs to the verified fix commit and deploy the PK binaries locally.

## Capabilities
### New Capabilities
- learning-changed-paths: bounded, non-mutating collection of top-level repository changes.
### Modified Capabilities
None.

## Impact
pk-learning-worker only; no CLI, queue schema, API or dependency changes. Downstream consumers are prometheus-skill-system and prometheus-skills-mini. Exact-commit pins, no release/tag or merge.
