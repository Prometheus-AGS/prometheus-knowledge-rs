//! Learning jobs carry the authoring agent's identity, and the worker must turn
//! it into surreal-memory scope keys (never a null `agent_id`), and must commit
//! the prompt snapshot so `pk context` can recall the session record.

use serde_json::{json, Value};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn transcript(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join(format!("transcript-{}.jsonl", text.len()));
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

fn queue_job(queue: &Path, event_id: &str, project: &Path, transcript: &Path, extra: Value) {
    let mut job = json!({
        "schemaVersion": 2,
        "eventId": event_id,
        "eventType": "subagent_stop",
        "harness": "fixture",
        "sessionId": "fixture-session",
        "projectRoot": project,
        "transcriptPath": transcript,
        "capturedAt": "2026-10-04T00:00:00Z",
        "payloadDigest": "fixture",
        "attempt": 0
    });
    for (key, value) in extra.as_object().unwrap() {
        job[key] = value.clone();
    }
    fs::write(
        queue.join("pending").join(format!("{event_id}.json")),
        serde_json::to_vec_pretty(&job).unwrap(),
    )
    .unwrap();
}

/// Every queued memory operation, in any state directory, keyed by operation id.
fn memory_operations(queue: &Path) -> Vec<Value> {
    let mut operations = Vec::new();
    for state in fs::read_dir(queue.join("memory")).unwrap().flatten() {
        if !state.path().is_dir() {
            continue;
        }
        for file in fs::read_dir(state.path()).unwrap().flatten() {
            if file.path().extension().and_then(|value| value.to_str()) == Some("json") {
                operations.push(serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap());
            }
        }
    }
    operations
}

#[test]
fn jobs_become_attributed_memory_operations_and_recallable_snapshot_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("pending")).unwrap();
    fs::create_dir_all(project.join(".git")).unwrap();

    let attributed = "a".repeat(64);
    let unattributed = "b".repeat(64);
    queue_job(
        &queue,
        &attributed,
        &project,
        &transcript(
            fixture.path(),
            "LESSON: anchor matchers to the plugin prefix.",
        ),
        json!({"projectId": "project:fixture", "teamId": "tlm-fixture", "roleId": "api-dev"}),
    );
    queue_job(
        &queue,
        &unattributed,
        &project,
        &transcript(fixture.path(), "Main-thread turn with no team role."),
        json!({}),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
        .env("HOME", &home)
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

    let operations = memory_operations(&queue);
    let by_id = |id: &str| {
        operations
            .iter()
            .find(|operation| operation["operationId"] == id)
            .unwrap_or_else(|| panic!("no memory operation {id}: {operations:#?}"))
    };
    let attributed_args = &by_id(&attributed)["arguments"];
    assert_eq!(attributed_args["user_id"], "project:fixture");
    assert_eq!(attributed_args["agent_id"], "tlm-fixture/api-dev");

    let unattributed_args = &by_id(&unattributed)["arguments"];
    assert_eq!(unattributed_args["agent_id"], "@project");
    let user_id = unattributed_args["user_id"].as_str().unwrap();
    assert!(user_id.starts_with("project:"), "{unattributed_args:#}");

    for operation in &operations {
        for key in ["user_id", "agent_id"] {
            assert!(
                operation["arguments"][key].is_string(),
                "operation with a null {key}: {operation:#}"
            );
        }
    }

    // The committed prompt snapshot is what `pk context` reads.
    let snapshot =
        pk_store::read_prompt_snapshot(&project.join(".prometheus/knowledge"), "project")
            .expect("worker must commit a project prompt snapshot");
    let ids: Vec<&str> = snapshot
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    for event_id in [&attributed, &unattributed] {
        let expected = format!("karpathy-session-{}", &event_id[..16]);
        assert!(
            ids.contains(&expected.as_str()),
            "{expected} missing from snapshot {ids:?}"
        );
    }
}

#[test]
fn legacy_task_step_operations_are_submitted_with_non_null_scope_keys() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("memory/pending")).unwrap();
    fs::write(
        queue.join("memory/pending/legacy-step.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "operationId": "legacy-step",
            "method": "add_task_step",
            "arguments": {"stream": "legacy:fixture:phase", "description": "change-001"},
            "dependencies": [],
            "state": "pending",
            "queuedAt": "2026-10-04T00:00:00Z",
            "lastError": null,
            "receipt": null
        }))
        .unwrap(),
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let capture = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut bodies = Vec::new();
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 8192];
                    loop {
                        match stream.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(count) => {
                                request.extend_from_slice(&buffer[..count]);
                                let text = String::from_utf8_lossy(&request);
                                if let Some(split) = text.find("\r\n\r\n") {
                                    let length = text[..split]
                                        .lines()
                                        .find_map(|line| {
                                            line.to_ascii_lowercase()
                                                .strip_prefix("content-length:")
                                                .map(|value| {
                                                    value.trim().parse::<usize>().unwrap_or(0)
                                                })
                                        })
                                        .unwrap_or(0);
                                    if request.len() >= split + 4 + length {
                                        break;
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let text = String::from_utf8_lossy(&request).into_owned();
                    // Receipt lookup first: an unknown operation is 404, which
                    // makes the worker submit it. Refuse the submission itself
                    // so nothing is marked delivered.
                    let reply: &[u8] = if text.starts_with("GET /ready ") {
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 32\r\nconnection: close\r\n\r\n{\"capabilities\":{\"ledger\":true}}"
                    } else if text.starts_with("GET ") {
                        b"HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"
                    } else {
                        b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    };
                    let _ = stream.write_all(reply);
                    let done = text.contains("stream_name");
                    bodies.push(text);
                    if done {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
        bodies
    });

    let output = Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .env("PROMETHEUS_PROJECT_ID", "project:legacy-fixture")
        .env_remove("PROMETHEUS_LEARNING_QUEUE")
        .args(["--memory-url", &format!("http://{address}"), "run-once"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bodies = capture.join().unwrap();
    let submitted = bodies
        .iter()
        .find(|body| body.contains("stream_name"))
        .unwrap_or_else(|| panic!("no task-step submission captured: {bodies:#?}"));
    let json_start = submitted.find("\r\n\r\n").unwrap() + 4;
    let body: Value = serde_json::from_str(&submitted[json_start..]).unwrap();
    let arguments = &body["payload"];
    assert_eq!(arguments["agent_id"], "@project", "{body:#}");
    assert_eq!(arguments["user_id"], "project:legacy-fixture", "{body:#}");
}
