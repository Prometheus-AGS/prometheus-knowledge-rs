use axum::{
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration as TimeDelta, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
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
    // accept() with a deadline: a worker that exits without connecting must fail
    // this test, not hang it. A blocking accept() did exactly that on Windows CI.
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "the worker never connected to the memory server"
                    );
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        // An accepted socket inherits non-blocking mode on Windows and the BSDs.
        stream.set_nonblocking(false).unwrap();
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

    // Before the join, so a failed worker reports its stderr rather than a timeout.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
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

fn worker(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prometheus-learning-worker"))
        .env("HOME", home)
        .env("RUST_LOG", "error")
        .env_remove("PROMETHEUS_LEARNING_QUEUE")
        .env_remove("PROMETHEUS_LEARNING_STALE_AFTER")
        .args(args)
        .output()
        .unwrap()
}

fn succeed(home: &Path, args: &[&str]) -> Output {
    let output = worker(home, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn add_memory_arguments(operation_id: &str) -> Value {
    json!({
        "content": format!("stranded fixture {operation_id}"),
        "user_id": "project:test",
        "agent_id": null,
        "session_id": "fixture-session",
        "categories": ["karpathy", "session-learning"]
    })
}

fn payload_hash(arguments: &Value) -> String {
    Sha256::digest(serde_json::to_vec(arguments).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn planned_receipt(operation_id: &str, server_time: DateTime<Utc>) -> Value {
    json!({
        "operation_id": operation_id,
        "schema_version": 2,
        "kind": "add_memory",
        "payload_hash": payload_hash(&add_memory_arguments(operation_id)),
        "dependencies": [],
        "state": "planned",
        "blocked_by": [],
        "result": null,
        "error": null,
        "executor_generation": 1,
        "progress_seq": 3,
        "created_at": server_time.to_rfc3339(),
        "updated_at": server_time.to_rfc3339()
    })
}

/// Write an accepted operation. `bookkeeping` is merged into the record, so a
/// test can model either a legacy file or one the current worker wrote.
fn seed_accepted(queue: &Path, operation_id: &str, receipt: Value, bookkeeping: Value) -> PathBuf {
    let arguments = add_memory_arguments(operation_id);
    let mut record = json!({
        "schemaVersion": 2,
        "operationId": operation_id,
        "method": "add_memory",
        "payloadHash": payload_hash(&arguments),
        "arguments": arguments,
        "dependencies": [],
        "state": "accepted",
        "queuedAt": receipt["created_at"],
        "lastError": null,
        "receipt": receipt
    });
    for (key, value) in bookkeeping.as_object().unwrap() {
        record[key] = value.clone();
    }
    let path = queue
        .join("memory/accepted")
        .join(format!("{operation_id}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    path
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.insert(path.clone(), Vec::new());
                directories.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// A ledger that answers every lookup with the same non-terminal receipt,
/// the way a server that stranded the operation does.
struct StrandingLedger {
    url: String,
    lookups: Arc<AtomicUsize>,
    submissions: Arc<AtomicUsize>,
    _runtime: tokio::runtime::Runtime,
}

fn serve_stranding_ledger(receipt: Value) -> StrandingLedger {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let lookups = Arc::new(AtomicUsize::new(0));
    let submissions = Arc::new(AtomicUsize::new(0));
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let lookup_count = Arc::clone(&lookups);
    let submission_count = Arc::clone(&submissions);
    let lookup_receipt = receipt.clone();
    let app = Router::new()
        .route(
            "/api/v2/operations/{operation_id}",
            get(move || {
                lookup_count.fetch_add(1, Ordering::SeqCst);
                let receipt = lookup_receipt.clone();
                async move { Json(receipt) }
            }),
        )
        .route(
            "/api/v2/operations",
            post(move || {
                submission_count.fetch_add(1, Ordering::SeqCst);
                let receipt = receipt.clone();
                async move { Json(receipt) }
            }),
        )
        .route(
            "/ready",
            get(|| async { Json(json!({"capabilities":{"ledger":true}})) }),
        );
    runtime.spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    StrandingLedger {
        url,
        lookups,
        submissions,
        _runtime: runtime,
    }
}

#[test]
fn stranded_operation_is_stale_backed_off_and_not_reported_delivered() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let queue = home.join(".prometheus/learning-queue");
    let operation_id = "stranded-planned";
    // A record written before the worker kept bookkeeping, whose receipt the
    // server last changed twelve days ago.
    let receipt = planned_receipt(operation_id, Utc::now() - TimeDelta::days(12));
    seed_accepted(&queue, operation_id, receipt.clone(), json!({}));
    let ledger = serve_stranding_ledger(receipt);
    let run = ["--memory-url", ledger.url.as_str(), "run-once"];

    // First poll: still in flight, already past the 6h default threshold.
    succeed(&home, &run);
    let status = read_json(&queue.join("status.json"));
    assert_eq!(status["memoryDelivered"], 0, "{status}");
    assert_eq!(status["memoryInFlight"], 1, "{status}");
    assert_eq!(status["memoryStale"], 1, "{status}");
    assert_eq!(status["memoryAwaitingReconciliation"], 0, "{status}");
    assert_eq!(status["staleAfterSeconds"], 6 * 3600, "{status}");
    assert!(
        status["oldestAcceptedAgeSeconds"].as_i64().unwrap() > 11 * 86_400,
        "{status}"
    );
    assert_eq!(ledger.lookups.load(Ordering::SeqCst), 1);

    // Second poll: the receipt did not move, so the record backs off.
    succeed(&home, &run);
    let status = read_json(&queue.join("status.json"));
    assert_eq!(status["memoryDelivered"], 0, "{status}");
    assert_eq!(status["memoryStale"], 1, "{status}");
    assert_eq!(ledger.lookups.load(Ordering::SeqCst), 2);
    let record = read_json(
        &queue
            .join("memory/accepted")
            .join(format!("{operation_id}.json")),
    );
    assert_eq!(record["unchangedPolls"], 1, "{record}");
    assert!(record["nextPollAt"].is_string(), "{record}");
    assert_eq!(record["lastReceiptState"], "planned", "{record}");
    assert_eq!(record["lastReceiptProgressSeq"], 3, "{record}");

    // Third run inside the backoff window: the record is not sent again.
    succeed(&home, &run);
    let status = read_json(&queue.join("status.json"));
    assert_eq!(status["memoryDelivered"], 0, "{status}");
    assert_eq!(status["memoryDeferred"], 1, "{status}");
    assert_eq!(status["memoryStale"], 1, "{status}");
    assert_eq!(ledger.lookups.load(Ordering::SeqCst), 2);
    assert_eq!(ledger.submissions.load(Ordering::SeqCst), 0);

    let report = stdout_json(&succeed(&home, &["status", "--json"]));
    assert_eq!(report["memoryAccepted"], 1, "{report}");
    assert_eq!(report["memoryStale"], 1, "{report}");
    assert_eq!(report["memoryStalled"], 0, "{report}");
}

#[test]
fn quarantine_dry_run_is_read_only_and_quarantine_release_round_trips() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let queue = home.join(".prometheus/learning-queue");
    let now = Utc::now();
    let stale = seed_accepted(
        &queue,
        "stale-operation",
        planned_receipt("stale-operation", now - TimeDelta::days(12)),
        json!({
            "firstAcceptedAt": (now - TimeDelta::days(12)).to_rfc3339(),
            "lastReceiptState": "planned",
            "lastReceiptProgressSeq": 3,
            "lastReceiptChangeAt": (now - TimeDelta::days(12)).to_rfc3339(),
            "unchangedPolls": 9
        }),
    );
    let fresh = seed_accepted(
        &queue,
        "fresh-operation",
        planned_receipt("fresh-operation", now),
        json!({
            "firstAcceptedAt": now.to_rfc3339(),
            "lastReceiptState": "planned",
            "lastReceiptProgressSeq": 3,
            "lastReceiptChangeAt": now.to_rfc3339()
        }),
    );
    let original = read_json(&stale);
    // The first invocation settles the queue layout; everything after the
    // dry run must be byte-identical to this.
    succeed(&home, &["status", "--json"]);
    let before = snapshot(&queue);

    let dry_run = stdout_json(&succeed(&home, &["quarantine", "--dry-run", "--json"]));
    let listed = dry_run["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| operation["operationId"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(listed, vec!["stale-operation".to_owned()], "{dry_run}");
    assert_eq!(dry_run["dryRun"], true, "{dry_run}");
    assert_eq!(before, snapshot(&queue), "a dry run must change nothing");

    let quarantined = stdout_json(&succeed(&home, &["quarantine", "--json"]));
    assert_eq!(quarantined["operations"].as_array().unwrap().len(), 1);
    assert!(!stale.exists());
    assert!(fresh.exists());
    let stalled = queue.join("memory/stalled/stale-operation.json");
    assert_eq!(read_json(&stalled)["state"], "stalled");
    let report = stdout_json(&succeed(&home, &["status", "--json"]));
    assert_eq!(report["memoryStalled"], 1, "{report}");
    assert_eq!(report["memoryStale"], 0, "{report}");
    assert_eq!(report["memoryAccepted"], 1, "{report}");
    assert_eq!(report["memoryRejected"], 0, "{report}");

    let unknown = worker(&home, &["release", "no-such-operation"]);
    assert!(!unknown.status.success());
    assert!(stalled.exists(), "a failed release must move nothing");

    succeed(&home, &["release", "--all"]);
    assert!(!stalled.exists());
    let released = read_json(&stale);
    assert_eq!(released["state"], "accepted");
    assert_eq!(released["operationId"], original["operationId"]);
    assert_eq!(released["payloadHash"], original["payloadHash"]);
    assert_eq!(released["arguments"], original["arguments"]);
    assert_eq!(released["firstAcceptedAt"], original["firstAcceptedAt"]);
    // Zero is omitted from the record, like every unset bookkeeping field.
    assert_eq!(released["unchangedPolls"].as_u64().unwrap_or(0), 0);
    assert!(released["nextPollAt"].is_null());
    // The stale clock restarts, so the record gets a full window to deliver.
    let report = stdout_json(&succeed(&home, &["status", "--json"]));
    assert_eq!(report["memoryStale"], 0, "{report}");
    assert_eq!(report["memoryStalled"], 0, "{report}");
    assert_eq!(report["memoryAccepted"], 2, "{report}");

    let manifests = fs::read_dir(queue.join("memory/manifests"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(manifests.len(), 2, "{manifests:?}");
    assert!(manifests.iter().any(|name| name.starts_with("quarantine-")));
    assert!(manifests.iter().any(|name| name.starts_with("release-")));
}
