## 1. Store reconciliation

- [x] 1.1 Canonicalize the wiki root once and preserve wiki-relative source identity; verify with the two-alias removal regression and the `pk-store` integration suite

## 2. Worker outage handling

- [x] 2.1 Add a finite memory request timeout and stop the memory pass on timeout or connection failure; verify the operation remains durable in the delayed-ledger regression
- [x] 2.2 Run the complete `pk-learning-worker` unit and integration suites and verify the real queue run completes local jobs before the bounded ledger failure
- [x] 2.3 Avoid rewriting already-normalized submitting operations during restart recovery; verify the durable file retains its inode
