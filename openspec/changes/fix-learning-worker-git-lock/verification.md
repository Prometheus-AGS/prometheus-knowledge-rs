# Verification — macOS boundary B4

The completed production change was verified with real worker processes, Git repositories and isolated homes/queues.

- `cargo test --locked -p pk-learning-worker --test git_status --test worker --test attribution`: PASS, 13 tests (6 Git status, 5 worker, 2 attribution).
- The same compiled git_status integration executable with `PK_GIT_STATUS_WORKER_BIN` set to the preserved original installed worker: expected negative control, 3 passed / 3 failed. The old worker rewrote index bytes during stat refresh, included a dirty submodule, and omitted the timeout warning. Existing path reporting, 40-path cap and lock sentinel preservation passed.
- The fixed worker's real Git timeout (>1 MiB of status output) returned after the five-second deadline, emitted the warning, left no index.lock, and allowed the next git add.
- Limitation: this fixture did not demonstrate the original worker leaving a stale lock. Git may finish its optional refresh before blocking on stdout. The direct index-byte negative control distinguishes the unsafe optional write without claiming reproduction of the reporter's exact timing race.
- `git diff --check`: PASS. Changed Rust files formatted with rustfmt. No dependency or public API changes; no additional security hardening.

Full workspace tests, Clippy and cross-platform execution are outside this focused packet and are not claimed. Local release installation and consumer pin receipts are recorded separately after completion.

## Delivery and local release boundary

- Independent artifact review found no actionable findings in the worker or either consumer pin.
- Exact production fix: 99c2a13dc69992b561876852350aab06de4b2948. PK PR: https://github.com/Prometheus-AGS/prometheus-knowledge-rs/pull/41.
- Full pack PR: https://github.com/Prometheus-AGS/prometheus-skill-system/pull/246. Its submodule pin checker passed.
- Mini PR: https://github.com/Prometheus-AGS/prometheus-skills-mini/pull/93. Its versions parser/comparator passed against real staged gitlinks, Node engine and image contracts. The operator approved this specific versions.toml update.
- `cargo build --release --locked -p pk-cli -p pk-cherry -p pk-learning-worker`: PASS. Installed all three binaries atomically with ad-hoc signatures; version remains 1.11.0.
- Saved previous binaries, existing launch-agent definitions and the quiesced queue under the host's `~/.prometheus/backups/pk-issue-40-20261123`; installation.json records source SHA and installed hashes.
- Reloaded only ai.prometheus.pk-cherry and ai.prometheus.learning-worker, retaining original service definitions and data roots. Cherry health and ready returned HTTP 200, 935 pages indexed/on disk, zero parse failures, active watcher. The scheduled worker completed its first run with exit 0 and no lastError.
- Same six real-Git integration scenarios against the installed release worker: PASS, including timeout then successful git add.
- No release tags or PR merges performed. This closeout is documentation only; consumer pins remain the exact production-fix commit above.
