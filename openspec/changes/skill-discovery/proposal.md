## Why

The learning worker reads only the final assistant message of a session (`build_session_packet`). A workflow that several sessions repeat, and a skill that users keep correcting after it runs, are therefore invisible. The skill system's team-aware learning design (§6) wants both surfaced as human-confirmed skill candidates, and `pk candidates accept --kind skill` currently refuses with "not yet supported".

Changing what `pk candidates accept` does is a frozen-surface change (the `pk` CLI), and the worker gains a new on-disk index and candidate directory contents. This proposal names the consumers.

## What Changes

- **Whole-transcript parsing (new, additive).** `prometheus-learning-worker` reads the whole transcript JSONL of each processed job, bounded to 8 MiB, tolerating malformed lines. It extracts user prompts, `Write`/`Edit` artifacts under `docs/`, `reports/` or ending `.md`, Bash commands, the tool sequence, `Skill` invocations (and slash-command echoes), and the corrections a user makes after a skill ran. The session packet and the promotion detector are unchanged.
- **Skill discovery in `run-once` (new, additive).** After the promotion detector, the worker:
  - appends one workflow fingerprint per session (prompt word-trigrams plus tool-sequence 3-grams, with skills used, corrections, team and role) to `~/.prometheus/learning-index/workflows.jsonl`, idempotent on `sha256(projectId, sessionId)`;
  - proposes a **new-skill candidate** when a fingerprint cluster has at least 3 sessions or at least 2 projects and no skill covers it (a skill covers a workflow when it was invoked in at least half the sessions that show it), written to `~/.prometheus/skill-candidates/pending/<id>.json` with its evidence;
  - proposes a **skill-update candidate** when users corrected a skill at least twice after it ran, attributed to the job's `teamId`/`roleId` (when present), written to the same directory and logged in the format `propose-skill-update.sh` uses under `~/.prometheus/skill-updates/` (`pending.log` and a placeholder `<skill>-<date>.diff`).
- **`pk candidates accept --kind skill <id> [--update <skill>]` (behaviour change).** It no longer fails. It prints the `/pmpo-skill-creator create "<summary>"` invocation (or `/pmpo-skill-creator --update <skill>` for an update candidate or when `--update` is given) and the evidence path, then moves the candidate to `accepted/`. It never creates or edits a skill. `list` and `reject` are unchanged. `--update` is a new optional flag, additive.

## Consumers

| Consumer | Affected surface | Effect |
|---|---|---|
| prometheus-skill-system (`/pmpo-skill-creator`, `propose-skill-update.sh`, `kbd-open.sh`, reflector role) | `~/.prometheus/skill-candidates/`, `~/.prometheus/skill-updates/`, `pk candidates --kind skill` | Intended consumer. A human runs the printed invocation. `pending.log` lines gain an optional trailing `role=<team>/<role>` token; the leading `<skill>::<date> hits=N` fields and the `Run:` hint line are unchanged. Needs a pin bump; handoff note below. |
| prometheus-skills-mini | `pk ingest` | Unchanged. |
| forge-rs | `pk ingest` (shells out) | Unchanged. |
| prometheus-cli | pk-core / pk-store / pk-librarian crates | No public API change. |
| prometheus doctor | `status.json` | No field added. `lastError` may carry a `skill discovery: …` message. |
| surreal-memory-server | `/api/v2/operations` | Unchanged: skill discovery queues no memory operation. |
| Learning-queue layout, job schemaVersion 2 | Job schema | Unchanged. `teamId`/`roleId` were already read (#32). |

## Capabilities

### New Capabilities
- `skill-discovery`: whole-transcript extraction, workflow fingerprinting, new-skill and skill-update candidates, and human-confirmed acceptance that prints an invocation.

## Impact

- **Code:** `pk-learning-worker/src/transcript.rs` and `skill_candidates.rs` (new), `pk-learning-worker/src/main.rs`, `pk-cli/src/candidates.rs`.
- **Tests:** `pk-learning-worker/tests/skill_discovery.rs` (new), `pk-cli/tests/candidates.rs` (the "skill accept is not yet supported" assertion is replaced).
- **Dependencies:** none added. `Cargo.lock` is unchanged.
- **Downstream handoff (prometheus-skill-system):** bump the pk pin to the release containing this change. Document `pk candidates accept --kind skill` and `--update` in `docs/guide/13-tools-reference.md`. Point the reflector role and `kbd-open.sh` at `~/.prometheus/skill-candidates/pending`.
