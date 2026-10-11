## ADDED Requirements

### Requirement: The worker reads the whole transcript
`prometheus-learning-worker` SHALL parse the whole transcript JSONL of each processed job, reading at most 8 MiB and skipping malformed lines, and extract user prompts, `Write`/`Edit` artifacts under `docs/`, `reports/` or ending `.md`, Bash commands, `Skill` invocations, and the user corrections that follow a skill. Tool results and harness messages SHALL NOT count as prompts.

#### Scenario: A torn line does not stop the parse
- **WHEN** a transcript contains a truncated JSON line between valid lines
- **THEN** the lines after it are still extracted

### Requirement: The worker fingerprints sessions
After processing jobs, `run-once` SHALL append one record per newly seen session to `~/.prometheus/learning-index/workflows.jsonl`, carrying prompt word-trigram and tool-sequence 3-gram fingerprints, skills used, corrections, team id and role id. Re-processing the same session SHALL NOT append a second record.

#### Scenario: Re-running indexes nothing new
- **WHEN** the worker runs again with no new jobs
- **THEN** `workflows.jsonl` is byte-identical

### Requirement: A repeated uncovered workflow becomes a new-skill candidate
The worker SHALL write a candidate to `~/.prometheus/skill-candidates/pending/<id>.json`, atomically, with an evidence list, when a workflow fingerprint cluster has at least 3 sessions or at least 2 projects and no skill invoked in at least half of its sessions.

#### Scenario: Three report sessions in two projects
- **WHEN** three sessions with a similar report-writing prompt and tool sequence ran in two projects and invoked no skill
- **THEN** exactly one pending candidate with `candidateType: new-skill` exists, citing three evidence entries and two project ids

#### Scenario: An existing skill covers the workflow
- **WHEN** the same three sessions each invoked the same skill
- **THEN** no new-skill candidate is written

### Requirement: Post-skill corrections become an attributed update candidate
The worker SHALL write a `candidateType: skill-update` candidate when users corrected a skill at least twice after it ran, naming the skill and carrying the job's `teamId` and `roleId` when present, and SHALL log it in the `propose-skill-update.sh` format under `~/.prometheus/skill-updates/`.

#### Scenario: Skill(kbd-plan) followed by corrections
- **WHEN** a session run by team `kbd-team` role `planner` invoked `kbd-plan` and the user then corrected the result twice
- **THEN** exactly one update candidate exists with `skillName: kbd-plan`, `teamId: kbd-team`, `roleId: planner`, and `pending.log` holds a `kbd-plan::<date>` marker and a `/pmpo-skill-creator --update kbd-plan` hint

#### Scenario: A single correction is not enough
- **WHEN** a user corrected a skill once
- **THEN** no update candidate is written

### Requirement: Skill discovery is idempotent
Re-running the worker SHALL NOT create a second candidate for the same cluster or skill, SHALL NOT rewrite a pending candidate whose evidence is unchanged, SHALL NOT append to `pending.log` again, and SHALL NOT re-propose a candidate that was accepted or rejected.

#### Scenario: Re-run adds none
- **WHEN** the worker runs a second time with no new jobs
- **THEN** the pending directory holds the same files, byte-identical

### Requirement: Accepting a skill candidate prints an invocation
`pk candidates accept --kind skill <id>` SHALL print the `/pmpo-skill-creator` invocation (`--update <skill>` for an update candidate or when `--update <skill>` is given, otherwise `create "<summary>"`) and the evidence path, move the candidate to `accepted/`, and SHALL NOT create or modify a skill. A skill name that is not a plain file name SHALL be refused and leave the candidate in place.

#### Scenario: Accept a new-skill candidate
- **WHEN** `pk candidates accept <id> --kind skill` runs against a pending new-skill candidate
- **THEN** stdout contains `/pmpo-skill-creator create "<summary>"` and the accepted file path, the candidate is in `accepted/`, and no skills directory is created

#### Scenario: Accept an update candidate
- **WHEN** it runs against a pending skill-update candidate for `kbd-plan`
- **THEN** stdout contains `/pmpo-skill-creator --update kbd-plan`

## MODIFIED Requirements

### Requirement: Skill candidates are listed but not yet accepted
`pk candidates --kind skill` SHALL list, accept (per the requirement above) and reject entries under `~/.prometheus/skill-candidates/`. The earlier "not yet supported" refusal no longer applies.
