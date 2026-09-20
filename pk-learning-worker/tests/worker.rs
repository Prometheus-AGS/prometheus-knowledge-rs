use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::Duration,
};

#[test]
fn accepts_a_valid_receipt_after_the_previous_ten_second_ceiling() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let queue = home.join(".prometheus/learning-queue");
    fs::create_dir_all(queue.join("memory/submitting")).unwrap();

    let operation_id = "delayed-valid-receipt";
    let arguments = json!({
        "content": "measured cold lookup fixture",
        "user_id": "project:test",
        "agent_id": null,
        "session_id": "fixture-session",
        "categories": ["karpathy", "session-learning"]
    });
    let payload_hash = Sha256::digest(serde_json::to_vec(&arguments).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    fs::write(
        queue
            .join("memory/submitting")
            .join(format!("{operation_id}.json")),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 2,
            "operationId": operation_id,
            "method": "add_memory",
            "arguments": arguments,
            "dependencies": [],
            "payloadHash": payload_hash,
            "state": "submitting",
            "queuedAt": "2026-09-20T00:00:00Z",
            "lastError": null,
            "receipt": null
        }))
        .unwrap(),
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receipt_hash = payload_hash.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let count = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(
            request.starts_with(&format!("GET /api/v2/operations/{operation_id} ")),
            "{request}"
        );
        thread::sleep(Duration::from_millis(10_500));
        let body = serde_json::to_string(&json!({
            "operation_id": operation_id,
            "schema_version": 2,
            "kind": "add_memory",
            "payload_hash": receipt_hash,
            "dependencies": [],
            "state": "committed",
            "blocked_by": [],
            "result": {"id": "memory:delayed-valid-receipt"},
            "error": null,
            "executor_generation": 1,
            "progress_seq": 1,
            "created_at": "2026-09-20T00:00:00Z",
            "updated_at": "2026-09-20T00:00:01Z"
        }))
        .unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        stream.flush().unwrap();
    });

    let output = Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args(["--memory-url", &format!("http://{address}"), "run-once"])
        .output()
        .unwrap();
    server.join().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(queue
        .join("memory/completed")
        .join(format!("{operation_id}.json"))
        .exists());
    assert_eq!(
        json!(1),
        serde_json::from_slice::<serde_json::Value>(&fs::read(queue.join("status.json")).unwrap())
            .unwrap()["memoryDelivered"]
    );
}

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
