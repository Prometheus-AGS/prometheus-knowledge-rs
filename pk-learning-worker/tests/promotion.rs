//! The promotion detector: a lesson that recurs in two projects becomes exactly
//! one pending promotion candidate carrying one evidence entry per project, and
//! re-running the worker proposes nothing new.

use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const LESSON: &str =
    "LESSON: anchor hook matchers to the plugin prefix so renamed agents still match their hooks";

fn transcript(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(format!("transcript-{name}.jsonl"));
    fs::write(
        &path,
        serde_json::to_string(&json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    path
}

fn queue_job(queue: &Path, event_id: &str, project: &Path, transcript: &Path) {
    let job = json!({
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
    fs::write(
        queue.join("pending").join(format!("{event_id}.json")),
        serde_json::to_vec_pretty(&job).unwrap(),
    )
    .unwrap();
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

fn json_files(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
}

#[test]
fn a_lesson_recurring_in_two_projects_yields_one_candidate_and_reruns_add_none() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("pending")).unwrap();
    let mut projects = Vec::new();
    for name in ["alpha", "beta"] {
        let project = fixture.path().join(name);
        fs::create_dir_all(project.join(".git")).unwrap();
        projects.push(project);
    }
    queue_job(
        &queue,
        &"a".repeat(64),
        &projects[0],
        &transcript(fixture.path(), "alpha", LESSON),
    );
    queue_job(
        &queue,
        &"b".repeat(64),
        &projects[1],
        &transcript(fixture.path(), "beta", LESSON),
    );

    run_worker(&home);

    let pending_dir = home.join(".prometheus/promotion-candidates/pending");
    let pending = json_files(&pending_dir);
    assert_eq!(pending.len(), 1, "expected one candidate: {pending:?}");
    let candidate: Value = serde_json::from_slice(&fs::read(&pending[0]).unwrap()).unwrap();
    assert_eq!(candidate["kind"], "promotion", "{candidate:#}");
    assert_eq!(candidate["state"], "pending", "{candidate:#}");
    let reasons = candidate["reasons"].as_array().unwrap();
    assert!(
        reasons.iter().any(|reason| reason == "recurrence"),
        "{candidate:#}"
    );
    let evidence = candidate["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 2, "{candidate:#}");
    let projects_cited: std::collections::BTreeSet<&str> = evidence
        .iter()
        .map(|item| item["projectId"].as_str().unwrap())
        .collect();
    assert_eq!(projects_cited.len(), 2, "{candidate:#}");
    assert_eq!(
        pending[0].file_stem().unwrap().to_str().unwrap(),
        candidate["id"].as_str().unwrap()
    );

    let index = home.join(".prometheus/learning-index/lessons.jsonl");
    let indexed = fs::read_to_string(&index).unwrap();
    assert_eq!(indexed.lines().count(), 2, "{indexed}");
    let before = fs::read(&pending[0]).unwrap();

    run_worker(&home);

    let after_pending = json_files(&pending_dir);
    assert_eq!(after_pending, pending, "a re-run must not add candidates");
    assert_eq!(
        fs::read(&pending[0]).unwrap(),
        before,
        "an unchanged candidate is not rewritten"
    );
    assert_eq!(fs::read_to_string(&index).unwrap(), indexed);
}
