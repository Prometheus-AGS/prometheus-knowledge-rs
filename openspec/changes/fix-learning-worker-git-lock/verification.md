# Verification — macOS boundary B4

The completed production change was verified with real worker processes, Git repositories and isolated homes/queues.

- `cargo test --locked -p pk-learning-worker --test git_status --test worker --test attribution`: PASS, 13 tests (6 Git status, 5 worker, 2 attribution).
- The same compiled git_status integration executable with `PK_GIT_STATUS_WORKER_BIN` set to the preserved original installed worker: expected negative control, 3 passed / 3 failed. The old worker rewrote index bytes during stat refresh, included a dirty submodule, and omitted the timeout warning. Existing path reporting, 40-path cap and lock sentinel preservation passed.
- The fixed worker's real Git timeout (>1 MiB of status output) returned after the five-second deadline, emitted the warning, left no index.lock, and allowed the next git add.
- Limitation: this fixture did not demonstrate the original worker leaving a stale lock. Git may finish its optional refresh before blocking on stdout. The direct index-byte negative control distinguishes the unsafe optional write without claiming reproduction of the reporter's exact timing race.
- `git diff --check`: PASS. Changed Rust files formatted with rustfmt. No dependency or public API changes; no additional security hardening.

Full workspace tests, Clippy and cross-platform execution are outside this focused packet and are not claimed. Local release installation and consumer pin receipts are recorded separately after completion.
