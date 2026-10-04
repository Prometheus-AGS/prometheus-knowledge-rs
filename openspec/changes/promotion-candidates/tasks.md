## 1. Promotion detector (learning-worker-engineer)
- [x] 1.1 `pk-learning-worker/src/promotion.rs`: lesson extraction, trigram fingerprints, `lessons.jsonl` index, greedy clustering, candidate proposal with evidence.
- [x] 1.2 Return observations from `process_job` and run the detector after the job loop in `run_once` (`pk-learning-worker/src/main.rs`).
- [x] 1.3 `pk-learning-worker/tests/promotion.rs`: two projects with one lesson yield one candidate with two evidence entries, and a re-run changes nothing.

## 2. `pk candidates` (surfaces-engineer)
- [x] 2.1 `pk-cli/src/candidates.rs` + the `Candidates` subcommand in `pk-cli/src/main.rs`: list, accept (promotion), reject, `--kind skill` list/reject with accept refused.
- [x] 2.2 `pk-cli/tests/candidates.rs`: accept writes the shared snapshot, queues one `@global` op with a valid payloadHash, and moves the file; reject moves the file; skill accept exits non-zero.

## 3. Release and handoff (knowledge-lead)
- [ ] 3.1 Verifier gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- [ ] 3.2 Downstream handoff note for prometheus-skill-system (pin, tools reference, `kbd-open.sh`/reflector surfacing).
