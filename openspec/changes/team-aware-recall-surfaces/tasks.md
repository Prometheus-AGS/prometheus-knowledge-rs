Records work that already merged. The evidence is each PR's integration test.

## 1. Context scoring (#31)
- [x] 1.1 Score all entries and cap after ranking (`pk-cli/src/main.rs::run_context`).
- [x] 1.2 `pk-cli/tests/context_scoring.rs`: 3/3. Negative control: the pre-fix binary returned `results=[]` and `candidate_count=128` on the 200-entry fixture.
- [x] 1.3 Update the `candidate_budget_is_shared_across_requested_scopes` assertion to the new semantics (design.md).

## 2. Worker attribution and snapshot (#32)
- [x] 2.1 Add the job identity fields and non-null scope keys. The legacy `add_task_step` form gets keys too.
- [x] 2.2 Commit the prompt snapshot after upsert.
- [x] 2.3 `pk-learning-worker/tests/attribution.rs`: 2/2. `worker.rs`: 5/5.

## 3. Ingest classification and context tags (#34)
- [x] 3.1 `--type` and `--tag` on ingest, via `CompileOverrides`.
- [x] 3.2 `--tag` on context.
- [x] 3.3 `pk-cli/tests/tags_and_type.rs`: 1/1, end to end against a local model endpoint.

## 4. Release 1.10.0
- [x] 4.1 Bump the workspace version to 1.10.0 and refresh `Cargo.lock` for the workspace members.
- [ ] 4.2 Maintainers confirm the `WikiEntry.sources` gate (design.md), then tag `v1.10.0` on the merge commit.
