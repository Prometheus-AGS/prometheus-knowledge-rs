## Where it runs

Inside `run-once`, after the promotion detector and before memory reconciliation, while the queue lock is held. `process_job` now returns the promotion observations and an optional `SessionObservation` (the job's project, session, team, role and transcript digest). The worker is the only writer of `workflows.jsonl` and `skill-candidates/pending/`. A discovery error is logged and recorded in `status.json`'s existing `lastError`; it never fails the run.

## Transcript digest

One pass over `File::take(8 MiB)`, split on newlines. A line that does not parse as JSON is skipped, which also absorbs the torn tail a `take` cut produces. Every list and string is capped (200 prompts of 1,000 chars, 200 commands, 2,000 tool steps, 50 skill uses).

- **User prompt:** a user message with text. A message made only of `tool_result` blocks, `isMeta` messages, and text starting with `<` (harness tags) are not prompts. A `<command-name>/x</command-name>` echo is a skill invocation, except for built-in commands.
- **Tool sequence:** one token per `tool_use`: the tool name, `Bash:<program>` for Bash, `Skill:<name>` for Skill. These are the 3-gram input.
- **Artifact:** `file_path` of a `Write`/`Edit`/`MultiEdit`/`NotebookEdit` use when it has a `docs/` or `reports/` segment or ends `.md`.
- **Correction:** a user prompt within the next 6 prompts after a skill (and before the next skill) that starts with a redirect word (`no,`, `stop`, ...) or contains a rejection phrase (`don't`, `wrong`, `instead`, `you forgot`, ...). Detection is lexical and deliberately simple.

**The uncomfortable case:** lexical correction detection has false positives ("don't forget to also add tests" is a request, not a rejection). Two guards bound it: updates need at least 2 corrections, and nothing is applied without a human running `pk candidates accept` and then `/pmpo-skill-creator --update`.

## Fingerprints and clustering

- **Prompt shingles:** word trigrams over the first 5 prompts, hashed with the first 8 bytes of SHA-256 (stable across Rust releases, like `lessons.jsonl`).
- **Tool shingles:** 3-grams over the tool sequence, same hash. A session needs 3 tool steps.
- **Similarity:** `0.5 * jaccard(prompt) + 0.5 * jaccard(tool)`, threshold 0.5. Two sessions that run the same tool sequence need only about 0.2 prompt overlap; sessions with the same words but different tools need 0.8.
- **Clustering:** greedy single pass over `(recordedAt, workflowId)`, each record joining the first cluster whose anchor it matches. New records sort last, so an anchor never moves. Candidate id: `skill-new-` plus 16 hex digits of SHA-256 over the anchor's workflow id.
- **Thresholds are the spec's:** at least 3 sessions or at least 2 projects. Read literally, two sessions in two projects qualify. The human gate absorbs the noise; if it proves too much, raise `MIN_SESSIONS`/`WORKFLOW_SIMILARITY`, not remove the gate.
- **Coverage:** the skills invoked in a cluster's sessions are counted per name. A skill used in at least half the sessions covers the workflow and suppresses the candidate. Names only; the worker does not read installed skills.

## Update candidates

Grouped by `(skill, teamId, roleId)` across the whole index, so repeated sessions by one role accumulate into one candidate with one evidence entry per session. Id: `skill-upd-` plus 16 hex digits of SHA-256 over those three. When a session has no team or role it is grouped as unattributed and the title says so. A plugin-qualified skill (`pack:kbd-plan`) is recorded by its last segment.

When an update candidate is written or its evidence grows, the worker appends the `propose-skill-update.sh` lines to `~/.prometheus/skill-updates/pending.log` (`<skill>::<date> hits=<n> role=<team>/<role>` plus the `Run: /pmpo-skill-creator --update <skill>` hint) unless that day's marker is already there, and creates the placeholder `<skill>-<date>.diff` if absent. Unlike the script, it does not require the skill to be installed or learning-log hits: the candidate's own evidence is the gate. A log failure warns and does not fail the run.

## Idempotency

As for promotion: a candidate whose id exists in `accepted/` or `rejected/` is skipped; a pending candidate is rewritten, atomically and keeping `createdAt`, only when its evidence set changed. `workflows.jsonl` is append-only and idempotent on `sha256(projectId, sessionId)`.

## Accept

`pk candidates accept --kind skill <id>` is read-only with respect to skills. For a candidate with `candidateType: skill-update` (or `--update <skill>`) it prints `/pmpo-skill-creator --update <skill>`; otherwise `/pmpo-skill-creator create "<summary>"` with quotes and control characters in the summary replaced by spaces. It then prints the evidence path (the candidate file's new place in `accepted/`) and the workflow index path, and moves the file. The skill name is validated against a file-name alphabet because it is echoed into a command line. `--update` on a promotion candidate is an error.
