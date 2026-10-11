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
    assert_eq!(
        operation["arguments"]["user_id"], "@global",
        "{operation:#}"
    );
    assert_eq!(
        operation["arguments"]["agent_id"], "@global",
        "{operation:#}"
    );
    assert_eq!(operation["arguments"]["content"], content);
    let expected_hash: String =
        Sha256::digest(serde_json::to_vec(&operation["arguments"]).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
    assert_eq!(operation["payloadHash"], expected_hash.as_str());

    // The candidate moved to accepted/.
    assert!(!pending.exists(), "pending file must be gone");
    let moved = home.join(format!(
        ".prometheus/promotion-candidates/accepted/{id}.json"
    ));
    let record: Value = serde_json::from_slice(&fs::read(&moved).unwrap()).unwrap();
    assert_eq!(record["state"], "accepted");

    // A second accept is refused, not duplicated.
    let again = pk(&home, &["candidates", "accept", id]);
    assert!(!again.status.success());
    assert_eq!(fs::read_dir(&outbox).unwrap().count(), 1);
}

#[test]
fn reject_moves_the_file_for_both_candidate_kinds() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let pending = write_candidate(&home, "promotion-candidates", "promo-reject", "A lesson.");
    let rejected = pk(
        &home,
        &["candidates", "reject", "promo-reject", "--reason", "noise"],
    );
    assert_ok(&rejected, "candidates reject");
    assert!(!pending.exists());
    assert!(home
        .join(".prometheus/promotion-candidates/rejected/promo-reject.json")
        .exists());

    write_candidate(&home, "skill-candidates", "skill-one", "A skill.");
    let listed = pk(&home, &["candidates", "list", "--kind", "skill"]);
    assert_ok(&listed, "candidates list --kind skill");
    assert!(String::from_utf8_lossy(&listed.stdout).contains("skill-one"));
    let rejected = pk(
        &home,
        &["candidates", "reject", "skill-one", "--kind", "skill"],
    );
    assert_ok(&rejected, "candidates reject --kind skill");
    assert!(home
        .join(".prometheus/skill-candidates/rejected/skill-one.json")
        .exists());
}

fn write_skill_candidate(home: &Path, id: &str, extra: Value) -> PathBuf {
    let path = home
        .join(".prometheus/skill-candidates/pending")
        .join(format!("{id}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut candidate = json!({
        "schemaVersion": 1,
        "id": id,
        "kind": "skill",
        "state": "pending",
        "reasons": ["sessions>=3"],
        "evidence": [{"workflowId": "w1", "projectId": "project:alpha"}],
        "createdAt": "2026-10-04T00:00:00Z",
        "updatedAt": "2026-10-04T00:00:00Z"
    });
    for (key, value) in extra.as_object().unwrap() {
        candidate[key] = value.clone();
    }
    fs::write(&path, serde_json::to_vec_pretty(&candidate).unwrap()).unwrap();
    path
}

#[test]
fn skill_accept_prints_the_create_invocation_and_never_creates_a_skill() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let pending = write_skill_candidate(
        &home,
        "skill-new-0001",
        json!({
            "candidateType": "new-skill",
            "title": "New skill: weekly status report",
            "summary": "Write the weekly \"status\" report"
        }),
    );

    let accept = pk(
        &home,
        &["candidates", "accept", "skill-new-0001", "--kind", "skill"],
    );
    assert_ok(&accept, "candidates accept --kind skill");
    let stdout = String::from_utf8_lossy(&accept.stdout);
    assert!(
        stdout.contains("/pmpo-skill-creator create \"Write the weekly  status  report\""),
        "{stdout}"
    );
    let accepted = home.join(".prometheus/skill-candidates/accepted/skill-new-0001.json");
    assert!(
        stdout.contains(&accepted.display().to_string()),
        "the evidence path is printed: {stdout}"
    );
    assert!(!pending.exists(), "pending file must be gone");
    let record: Value = serde_json::from_slice(&fs::read(&accepted).unwrap()).unwrap();
    assert_eq!(record["state"], "accepted");
    for created in [".claude/skills", ".codex/skills", ".prometheus/skills"] {
        assert!(
            !home.join(created).exists(),
            "accept must never create a skill ({created})"
        );
    }

    let again = pk(
        &home,
        &["candidates", "accept", "skill-new-0001", "--kind", "skill"],
    );
    assert!(!again.status.success(), "a second accept is refused");
}

#[test]
fn skill_accept_of_an_update_candidate_prints_the_update_invocation() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write_skill_candidate(
        &home,
        "skill-upd-0001",
        json!({
            "candidateType": "skill-update",
            "skillName": "kbd-plan",
            "teamId": "kbd-team",
            "roleId": "planner",
            "title": "Update kbd-plan"
        }),
    );
    let accept = pk(
        &home,
        &["candidates", "accept", "skill-upd-0001", "--kind", "skill"],
    );
    assert_ok(&accept, "accept update candidate");
    assert!(
        String::from_utf8_lossy(&accept.stdout).contains("/pmpo-skill-creator --update kbd-plan")
    );

    // `--update <skill>` forces the update form for a new-skill candidate.
    write_skill_candidate(
        &home,
        "skill-new-0002",
        json!({"candidateType": "new-skill", "summary": "Write a report"}),
    );
    let forced = pk(
        &home,
        &[
            "candidates",
            "accept",
            "skill-new-0002",
            "--kind",
            "skill",
            "--update",
            "learn-goal",
        ],
    );
    assert_ok(&forced, "accept --update");
    assert!(
        String::from_utf8_lossy(&forced.stdout).contains("/pmpo-skill-creator --update learn-goal")
    );

    // A skill name is echoed into a command line, so a path-like one is refused.
    let pending = write_skill_candidate(
        &home,
        "skill-new-0003",
        json!({"candidateType": "new-skill", "summary": "Write a report"}),
    );
    let bad = pk(
        &home,
        &[
            "candidates",
            "accept",
            "skill-new-0003",
            "--kind",
            "skill",
            "--update",
            "../evil",
        ],
    );
    assert!(!bad.status.success());
    assert!(
        pending.exists(),
        "a refused accept leaves the file in place"
    );
}
