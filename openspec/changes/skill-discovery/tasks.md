## 1. Whole-transcript extraction (learning-worker-engineer)
- [x] 1.1 `pk-learning-worker/src/transcript.rs`: bounded, lenient digest of prompts, artifacts, commands, tool sequence, skill uses and post-skill corrections.

## 2. Skill discovery (learning-worker-engineer)
- [x] 2.1 `pk-learning-worker/src/skill_candidates.rs`: `workflows.jsonl` fingerprints, new-skill clustering with coverage check, attributed update candidates, `propose-skill-update.sh`-format log.
- [x] 2.2 Return a `SessionObservation` from `process_job` and run discovery after the promotion detector in `run_once` (`pk-learning-worker/src/main.rs`).

## 3. `pk candidates accept --kind skill` (surfaces-engineer)
- [x] 3.1 `pk-cli/src/candidates.rs`: print the `/pmpo-skill-creator` invocation and evidence path, move to `accepted/`, add `--update <skill>`; never create a skill.

## 4. Tests (verifier)
- [x] 4.1 `pk-learning-worker/tests/skill_discovery.rs`: three sessions in two projects give one new-skill candidate; a covering skill suppresses it; `Skill(kbd-plan)` plus corrections gives one attributed update candidate; one correction gives none; re-runs add nothing.
- [x] 4.2 `pk-cli/tests/candidates.rs`: skill accept prints the create and update invocations, never creates a skill, refuses a path-like skill name; reject works for both kinds.

## 5. Release and handoff (knowledge-lead)
- [ ] 5.1 Verifier gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`.
- [ ] 5.2 Downstream handoff note for prometheus-skill-system (pin, tools reference, reflector/`kbd-open.sh` surfacing).
