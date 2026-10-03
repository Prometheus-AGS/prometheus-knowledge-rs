## 1. Store (core-engineer)

- [x] 1.1 RED: store tests for a preserved unparseable entry, two stale writers, frontmatter retention, and a dropped deleted page
- [x] 1.2 GREEN: lock, fresh scan, merge helper and atomic writes in `pk-store`

## 2. Worker boundary (learning-worker-engineer)

- [x] 2.1 Regression test: a worker `run-once` keeps unparseable entries and the `okf_version` block

## 1a. Verifier corrections (core-engineer)

- [x] 1.3 `append_log` takes the same lock (RED 9e1e962, GREEN 40442b6)
- [x] 1.4 Carry only lines for pages that failed to parse, and render structurally (findings F1, F2; RED f57db3f, GREEN 0e27913)

## 3. Verification (verifier)

- [ ] 3.1 Adversarial review and the serialized integration gate

## 4. Downstream (knowledge-lead)

- [x] 4.1 Skill-system handoff note: pin bump and same-version installs
