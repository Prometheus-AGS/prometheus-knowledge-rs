//! Skill discovery: the worker reads whole transcripts. Three similar
//! report-writing sessions across two projects become exactly one new-skill
//! candidate; one `Skill(kbd-plan)` followed by corrections becomes one update
//! candidate attributed to the role that ran it; re-running adds nothing.

use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn line(value: &Value) -> String {
    serde_json::to_string(value).unwrap() + "\n"
}

fn user(text: &str) -> String {
    line(&json!({"type": "user", "message": {"role": "user", "content": text}}))
}

fn tool_result(id: &str) -> String {
    line(&json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": id, "content": "ok"}
    ]}}))
}

fn tool_use(name: &str, input: Value) -> String {
    line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "text", "text": "Working on it."},
        {"type": "tool_use", "id": "toolu_1", "name": name, "input": input}
    ]}}))
}

fn assistant_text(text: &str) -> String {
    line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "text", "text": text}
    ]}}))
}

/// A report-writing session: read two sources, write a report, commit it.
fn report_transcript(project: &Path, topic: &str, slug: &str) -> String {
    let report = project.join(format!("reports/{slug}-status.md"));
    let mut text = String::new();
    text.push_str(&user(&format!(
        "Write the weekly status report for the {topic} workstream into reports/{slug}-status.md. \
         Cover risks, milestones and open decisions, and keep it under one page."
    )));
    text.push_str(&tool_use("Read", json!({"file_path": project.join("docs/plan.md")})));
    text.push_str(&tool_result("toolu_1"));
    text.push_str(&tool_use("Read", json!({"file_path": project.join("docs/notes.md")})));
    text.push_str(&tool_result("toolu_1"));
    // A torn line from a crash mid-write must not stop the parse.
    text.push_str("{\"type\": \"assistant\", \"message\": {\"role\": \"assis\n");
    text.push_str(&tool_use(
        "Write",
        json!({"file_path": report, "content": "# Status\n"}),
    ));
    text.push_str(&tool_result("toolu_1"));
    text.push_str(&tool_use(
        "Bash",
        json!({"command": format!("git add reports/{slug}-status.md")}),
    ));
    text.push_str(&tool_result("toolu_1"));
    text.push_str(&tool_use(
        "Bash",
        json!({"command": format!("git commit -m 'status report {slug}'")}),
    ));
    text.push_str(&tool_result("toolu_1"));
    text.push_str(&user("Add a short executive summary at the top."));
    text.push_str(&assistant_text("Done."));
    text
}

fn kbd_plan_transcript(project: &Path) -> String {
    let mut text = String::new();
    text.push_str(&user("Plan the auth refactor phase."));
    text.push_str(&tool_use("Skill", json!({"skill": "kbd-plan"})));
    text.push_str(&tool_result("toolu_1"));
    // The skill body is injected as a meta user message; it is not a prompt.
    text.push_str(&line(&json!({"type": "user", "isMeta": true,
        "message": {"role": "user", "content": "Base directory for this skill: /skills/kbd-plan"}})));
    text.push_str(&assistant_text("Here is a plan with five changes."));
    text.push_str(&user("No, don't split it into five changes; keep it to two."));
    text.push_str(&tool_use(
        "Write",
        json!({"file_path": project.join("docs/plan.md"), "content": "plan"}),
    ));
    text.push_str(&tool_result("toolu_1"));
    text.push_str(&user(
        "That's wrong, the plan must name the owning role for each task.",
    ));
    text.push_str(&assistant_text("Updated."));
    text
}

fn queue_job(
    queue: &Path,
    event_id: &str,
    project: &Path,
    transcript: &Path,
    role: Option<(&str, &str)>,
) {
    let mut job = json!({
        "schemaVersion": 2,
        "eventId": event_id,
        "eventType": "subagent_stop",
        "harness": "fixture",
        "sessionId": format!("session-{}", &event_id[..8]),
        "projectRoot": project,
        "transcriptPath": transcript,
        "capturedAt": "2026-10-04T00:00:00Z",
        "payloadDigest": "fixture",
        "attempt": 0
    });
    if let Some((team, role)) = role {
        job["teamId"] = json!(team);
        job["roleId"] = json!(role);
    }
    fs::write(
        queue.join("pending").join(format!("{event_id}.json")),
        serde_json::to_vec_pretty(&job).unwrap(),
    )
    .unwrap();
}

fn write_transcript(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(format!("transcript-{name}.jsonl"));
    fs::write(&path, text).unwrap();
    path
}

fn run_worker(home: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
        .env("HOME", home)
        .env("RUST_LOG", "error")
        .env_remove("PROMETHEUS_LEARNING_QUEUE")
        .args(["--memory-url", "http://127.0.0.1:1", "run-once"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn pending_candidates(home: &Path) -> Vec<(PathBuf, Value)> {
    let directory = home.join(".prometheus/skill-candidates/pending");
    let mut files: Vec<PathBuf> = fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            (path, value)
        })
        .collect()
}

fn of_type<'a>(candidates: &'a [(PathBuf, Value)], kind: &str) -> Vec<&'a Value> {
    candidates
        .iter()
        .map(|(_, value)| value)
        .filter(|value| value["candidateType"] == kind)
        .collect()
}

fn scratch_home(fixture: &Path) -> (PathBuf, PathBuf) {
    let home = fixture.join("home");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("pending")).unwrap();
    (home, queue)
}

fn project(fixture: &Path, name: &str) -> PathBuf {
    let project = fixture.join(name);
    fs::create_dir_all(project.join(".git")).unwrap();
    project
}

#[test]
fn three_similar_report_sessions_across_two_projects_yield_one_new_skill_candidate() {
    let fixture = tempfile::tempdir().unwrap();
    let (home, queue) = scratch_home(fixture.path());
    let alpha = project(fixture.path(), "alpha");
    let beta = project(fixture.path(), "beta");
    let sessions = [
        (&alpha, "payments migration", "payments", "a"),
        (&alpha, "search indexing", "search", "b"),
        (&beta, "mobile onboarding", "mobile", "c"),
    ];
    for (project, topic, slug, id) in sessions {
        let transcript = write_transcript(
            fixture.path(),
            slug,
            &report_transcript(project, topic, slug),
        );
        queue_job(&queue, &id.repeat(64), project, &transcript, None);
    }

    run_worker(&home);

    let pending = pending_candidates(&home);
    let new_skills = of_type(&pending, "new-skill");
    assert_eq!(new_skills.len(), 1, "expected one new-skill candidate: {pending:#?}");
    assert!(
        of_type(&pending, "skill-update").is_empty(),
        "no skill ran, so there is nothing to update: {pending:#?}"
    );
    let candidate = new_skills[0];
    assert_eq!(candidate["kind"], "skill", "{candidate:#}");
    assert_eq!(candidate["state"], "pending", "{candidate:#}");
    let reasons = candidate["reasons"].as_array().unwrap();
    for wanted in ["sessions>=3", "projects>=2", "no-covering-skill"] {
        assert!(reasons.iter().any(|r| r == wanted), "{wanted} in {candidate:#}");
    }
    let evidence = candidate["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 3, "{candidate:#}");
    let projects_cited: std::collections::BTreeSet<&str> = evidence
        .iter()
        .map(|item| item["projectId"].as_str().unwrap())
        .collect();
    assert_eq!(projects_cited.len(), 2, "{candidate:#}");
    for item in evidence {
        let artifacts = item["artifacts"].as_array().unwrap();
        assert!(
            artifacts
                .iter()
                .any(|a| a.as_str().unwrap().contains("reports/")),
            "the written report is cited as an artifact: {item:#}"
        );
    }
    assert_eq!(
        pending[0].0.file_stem().unwrap().to_str().unwrap(),
        candidate["id"].as_str().unwrap()
    );

    let index = home.join(".prometheus/learning-index/workflows.jsonl");
    let indexed = fs::read_to_string(&index).unwrap();
    assert_eq!(indexed.lines().count(), 3, "{indexed}");
    let before = fs::read(&pending[0].0).unwrap();

    // A re-run, and a run with a new unrelated job, add nothing.
    run_worker(&home);

    let after = pending_candidates(&home);
    assert_eq!(after.len(), pending.len(), "a re-run must not add candidates");
    assert_eq!(fs::read(&after[0].0).unwrap(), before, "unchanged, not rewritten");
    assert_eq!(fs::read_to_string(&index).unwrap(), indexed);
}

#[test]
fn a_skill_used_in_most_sessions_already_covers_the_workflow() {
    let fixture = tempfile::tempdir().unwrap();
    let (home, queue) = scratch_home(fixture.path());
    let alpha = project(fixture.path(), "alpha");
    let beta = project(fixture.path(), "beta");
    for (project, topic, slug, id) in [
        (&alpha, "payments migration", "payments", "a"),
        (&alpha, "search indexing", "search", "b"),
        (&beta, "mobile onboarding", "mobile", "c"),
    ] {
        // The same session, but each one invoked an existing report skill first.
        let mut text = user("Use the report skill.");
        text.push_str(&tool_use("Skill", json!({"skill": "status-report"})));
        text.push_str(&report_transcript(project, topic, slug));
        let transcript = write_transcript(fixture.path(), slug, &text);
        queue_job(&queue, &id.repeat(64), project, &transcript, None);
    }

    run_worker(&home);

    let pending = pending_candidates(&home);
    assert!(
        of_type(&pending, "new-skill").is_empty(),
        "an existing skill covers this workflow: {pending:#?}"
    );
}

#[test]
fn a_skill_followed_by_corrections_yields_one_update_candidate_for_its_role() {
    let fixture = tempfile::tempdir().unwrap();
    let (home, queue) = scratch_home(fixture.path());
    let alpha = project(fixture.path(), "alpha");
    let transcript = write_transcript(fixture.path(), "plan", &kbd_plan_transcript(&alpha));
    queue_job(
        &queue,
        &"d".repeat(64),
        &alpha,
        &transcript,
        Some(("kbd-team", "planner")),
    );

    run_worker(&home);

    let pending = pending_candidates(&home);
    let updates = of_type(&pending, "skill-update");
    assert_eq!(updates.len(), 1, "expected one update candidate: {pending:#?}");
    assert!(of_type(&pending, "new-skill").is_empty(), "{pending:#?}");
    let candidate = updates[0];
    assert_eq!(candidate["kind"], "skill", "{candidate:#}");
    assert_eq!(candidate["skillName"], "kbd-plan", "{candidate:#}");
    assert_eq!(candidate["teamId"], "kbd-team", "{candidate:#}");
    assert_eq!(candidate["roleId"], "planner", "{candidate:#}");
    let evidence = candidate["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 1, "{candidate:#}");
    assert_eq!(evidence[0]["teamId"], "kbd-team", "{candidate:#}");
    assert_eq!(evidence[0]["roleId"], "planner", "{candidate:#}");
    let corrections = evidence[0]["corrections"].as_array().unwrap();
    assert_eq!(corrections.len(), 2, "{candidate:#}");
    assert!(corrections[0].as_str().unwrap().starts_with("No, don't split"));

    // The propose-skill-update.sh format: marker line, hint line, placeholder diff.
    let updates_dir = home.join(".prometheus/skill-updates");
    let log = fs::read_to_string(updates_dir.join("pending.log")).unwrap();
    assert!(log.contains("kbd-plan::"), "{log}");
    assert!(log.contains("hits=1"), "{log}");
    assert!(log.contains("role=kbd-team/planner"), "{log}");
    assert!(
        log.contains("Run: /pmpo-skill-creator --update kbd-plan"),
        "{log}"
    );
    let diffs: Vec<_> = fs::read_dir(&updates_dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".diff"))
        .collect();
    assert_eq!(diffs.len(), 1);
    assert!(diffs[0]
        .file_name()
        .to_string_lossy()
        .starts_with("kbd-plan-"));

    let before = fs::read(&pending[0].0).unwrap();
    run_worker(&home);
    let after = pending_candidates(&home);
    assert_eq!(after.len(), 1, "a re-run adds nothing");
    assert_eq!(fs::read(&after[0].0).unwrap(), before);
    assert_eq!(
        fs::read_to_string(updates_dir.join("pending.log")).unwrap(),
        log,
        "the update log is not appended again"
    );
}

#[test]
fn a_single_correction_after_a_skill_is_not_enough() {
    let fixture = tempfile::tempdir().unwrap();
    let (home, queue) = scratch_home(fixture.path());
    let alpha = project(fixture.path(), "alpha");
    let mut text = user("Plan the auth refactor phase.");
    text.push_str(&tool_use("Skill", json!({"skill": "kbd-plan"})));
    text.push_str(&user("No, keep it to two changes."));
    text.push_str(&user("Looks good, thanks."));
    let transcript = write_transcript(fixture.path(), "plan", &text);
    queue_job(&queue, &"e".repeat(64), &alpha, &transcript, None);

    run_worker(&home);

    assert!(pending_candidates(&home).is_empty());
}
