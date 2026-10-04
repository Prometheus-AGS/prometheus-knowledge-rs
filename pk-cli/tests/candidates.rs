//! `pk candidates accept` promotes a pending lesson: it lands in the shared KB's
//! committed prompt snapshot, an `@global` add_memory operation is queued in the
//! learning-queue outbox (schemaVersion 2, payloadHash over the arguments), and
//! the candidate file moves to accepted/.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn pk(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(home)
        .env("HOME", home)
        .env("RUST_LOG", "error")
        .env_remove("PROMETHEUS_LEARNING_QUEUE")
        .env_remove("PK_KB_DIR")
        .args(args)
        .output()
        .unwrap()
}

fn write_candidate(home: &Path, kind_dir: &str, id: &str, content: &str) -> PathBuf {
    let path = home
        .join(".prometheus")
        .join(kind_dir)
        .join("pending")
        .join(format!("{id}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "id": id,
            "kind": "promotion",
            "state": "pending",
            "scope": "global",
            "reasons": ["recurrence"],
            "title": "Anchor hook matchers to the plugin prefix",
            "content": content,
            "tags": ["hooks"],
            "fingerprint": "f",
            "evidence": [
                {"lessonId": "l1", "projectId": "project:alpha", "projectRoot": "/alpha",
                 "source": "session:s1#0", "recordedAt": "2026-10-04T00:00:00Z", "similarity": 1.0},
                {"lessonId": "l2", "projectId": "project:beta", "projectRoot": "/beta",
                 "source": "session:s2#0", "recordedAt": "2026-10-04T00:00:00Z", "similarity": 1.0}
            ],
            "createdAt": "2026-10-04T00:00:00Z",
            "updatedAt": "2026-10-04T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn assert_ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accept_writes_the_shared_snapshot_queues_a_global_operation_and_moves_the_file() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let id = "promo-0123456789abcdef";
    let content = "Anchor hook matchers to the plugin prefix so renamed agents still match.";
    let pending = write_candidate(&home, "promotion-candidates", id, content);

    let listed = pk(&home, &["candidates", "list"]);
    assert_ok(&listed, "candidates list");
    assert!(String::from_utf8_lossy(&listed.stdout).contains(id));

    let accepted = pk(&home, &["candidates", "accept", id, "--kind", "promotion"]);
    assert_ok(&accepted, "candidates accept");

    // Shared KB entry, published through the committed prompt snapshot.
    let shared = home.join(".prometheus/knowledge/shared");
    let snapshot = pk_store::read_prompt_snapshot(&shared, "shared")
        .expect("accept must commit the shared prompt snapshot");
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.id.as_str() == format!("promoted-{id}"))
        .unwrap_or_else(|| panic!("promoted entry missing from shared snapshot"));
    assert_eq!(entry.content.trim(), content);

    // Exactly one @global add_memory operation, hash-bound to its arguments.
    let outbox = home.join(".prometheus/learning-queue/memory/pending");
    let operations: Vec<Value> = fs::read_dir(&outbox)
        .unwrap()
        .flatten()
        .filter(|file| file.path().extension().and_then(|v| v.to_str()) == Some("json"))
        .map(|file| serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap())
        .collect();
    assert_eq!(operations.len(), 1, "{operations:#?}");
    let operation = &operations[0];
    assert_eq!(operation["schemaVersion"], 2);
    assert_eq!(operation["method"], "add_memory");
    assert_eq!(operation["arguments"]["user_id"], "@global", "{operation:#}");
    assert_eq!(operation["arguments"]["agent_id"], "@global", "{operation:#}");
    assert_eq!(operation["arguments"]["content"], content);
    let expected_hash: String =
        Sha256::digest(serde_json::to_vec(&operation["arguments"]).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
    assert_eq!(operation["payloadHash"], expected_hash.as_str());

    // The candidate moved to accepted/.
    assert!(!pending.exists(), "pending file must be gone");
    let moved = home.join(format!(".prometheus/promotion-candidates/accepted/{id}.json"));
    let record: Value = serde_json::from_slice(&fs::read(&moved).unwrap()).unwrap();
    assert_eq!(record["state"], "accepted");

    // A second accept is refused, not duplicated.
    let again = pk(&home, &["candidates", "accept", id]);
    assert!(!again.status.success());
    assert_eq!(fs::read_dir(&outbox).unwrap().count(), 1);
}

#[test]
fn reject_moves_the_file_and_skill_accept_is_not_yet_supported() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let pending = write_candidate(&home, "promotion-candidates", "promo-reject", "A lesson.");
    let rejected = pk(&home, &["candidates", "reject", "promo-reject", "--reason", "noise"]);
    assert_ok(&rejected, "candidates reject");
    assert!(!pending.exists());
    assert!(home
        .join(".prometheus/promotion-candidates/rejected/promo-reject.json")
        .exists());

    let skill = write_candidate(&home, "skill-candidates", "skill-one", "A skill.");
    let listed = pk(&home, &["candidates", "list", "--kind", "skill"]);
    assert_ok(&listed, "candidates list --kind skill");
    assert!(String::from_utf8_lossy(&listed.stdout).contains("skill-one"));
    let accept = pk(&home, &["candidates", "accept", "skill-one", "--kind", "skill"]);
    assert!(!accept.status.success(), "skill accept must exit non-zero");
    assert!(String::from_utf8_lossy(&accept.stderr).contains("not yet supported"));
    assert!(skill.exists(), "an unsupported accept leaves the file in place");
}
