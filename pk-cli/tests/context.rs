use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn write_entry(base: &Path, id: &str, title: &str, token: &str) {
    let wiki = base.join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    fs::write(
        wiki.join(format!("{id}.md")),
        format!(
            "---\ntype: Reference\ntitle: {title}\ntags: [fixture]\n---\n\n{token} deterministic context fixture\n"
        ),
    )
    .unwrap();
}

#[test]
fn context_searches_committed_snapshots_with_candidate_and_byte_bounds() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    write_entry(
        &project.join(".prometheus/knowledge"),
        "project-fixture",
        "Project fixture",
        "projectuniquetoken",
    );
    write_entry(
        &home.join(".prometheus/knowledge/shared"),
        "shared-fixture",
        "Shared fixture",
        "shareduniquetoken",
    );
    write_entry(
        &home.join(".prometheus/knowledge"),
        "global-fixture",
        "Global fixture",
        "globaluniquetoken",
    );

    let snapshot = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args(["snapshot"])
        .output()
        .unwrap();
    assert!(
        snapshot.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot.stderr)
    );
    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args([
            "context",
            "projectuniquetoken shareduniquetoken globaluniquetoken",
            "--scope",
            "project",
            "--scope",
            "shared",
            "--scope",
            "global",
            "--limit",
            "8",
            "--max-candidates",
            "8",
            "--max-bytes",
            "4096",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["candidate_count"].as_u64().unwrap() <= 8);
    assert!(report["byte_count"].as_u64().unwrap() <= 4096);
    assert_eq!(report["snapshot_generations"].as_object().unwrap().len(), 3);
    let scopes = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["scope"].as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(scopes.len(), 3, "{report:#}");
}

#[test]
fn context_returns_partial_results_when_a_scope_is_missing() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    write_entry(
        &project.join(".prometheus/knowledge"),
        "available",
        "Available",
        "availabletoken",
    );
    let snapshot = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args(["snapshot", "--scope", "project"])
        .output()
        .unwrap();
    assert!(snapshot.status.success());
    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args([
            "context",
            "availabletoken",
            "--scope",
            "project",
            "--scope",
            "shared",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["results"].as_array().unwrap().len(), 1);
    assert_eq!(report["failures"].as_array().unwrap().len(), 1);
}

#[test]
fn candidate_budget_is_shared_across_requested_scopes() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    for index in 0..4 {
        write_entry(
            &project.join(".prometheus/knowledge"),
            &format!("irrelevant-{index}"),
            "Irrelevant",
            "unrelatedtoken",
        );
    }
    write_entry(
        &home.join(".prometheus/knowledge/shared"),
        "shared-target",
        "Shared target",
        "sharedbudgettoken",
    );
    let snapshot = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args(["snapshot", "--scope", "project", "--scope", "shared"])
        .output()
        .unwrap();
    assert!(snapshot.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args([
            "context",
            "sharedbudgettoken",
            "--scope",
            "project",
            "--scope",
            "shared",
            "--max-candidates",
            "2",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["candidate_count"], 2);
    assert_eq!(report["results"][0]["id"], "shared-target");
}
