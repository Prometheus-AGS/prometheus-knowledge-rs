use serde_json::json;
use std::{fs, process::Command};

#[test]
fn processes_a_job_once_and_preserves_ambiguous_memory_delivery() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("pending")).unwrap();
    fs::create_dir_all(project.join(".git")).unwrap();
    let transcript = fixture.path().join("transcript.jsonl");
    fs::write(
        &transcript,
        serde_json::to_string(&json!({
            "type":"assistant",
            "message":{"role":"assistant","content":[{"type":"text","text":"Verified durable worker behavior."}]}
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let event_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    fs::write(
        queue.join("pending").join(format!("{event_id}.json")),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion":2,
            "eventId":event_id,
            "eventType":"stop",
            "harness":"fixture",
            "sessionId":"fixture-session",
            "projectRoot":project,
            "transcriptPath":transcript,
            "capturedAt":"2026-08-03T00:00:00Z",
            "payloadDigest":"fixture",
            "attempt":0
        }))
        .unwrap(),
    )
    .unwrap();

    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
            .env("HOME", &home)
            .env("RUST_LOG", "error")
            .args(["--memory-url", "http://127.0.0.1:1", "run-once"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let wiki_entries = fs::read_dir(project.join(".prometheus/knowledge/wiki"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("karpathy-session-")
        })
        .count();
    assert_eq!(wiki_entries, 1);
    assert_eq!(fs::read_dir(queue.join("completed")).unwrap().count(), 1);
    assert_eq!(
        fs::read_dir(queue.join("memory/submitting"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(fs::read_dir(queue.join("memory/retry")).unwrap().count(), 0);
    let learning_log_path = fs::read_dir(home.join(".prometheus/learning-log"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .unwrap();
    let learning_log = fs::read_to_string(learning_log_path).unwrap();
    assert_eq!(learning_log.lines().count(), 1);
}
