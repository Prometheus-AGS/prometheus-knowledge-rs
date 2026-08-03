use serde_json::Value;
use std::{fs, path::Path, process::Command, time::Instant};

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
fn context_searches_all_scopes_within_the_hard_budget() {
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

    let started = Instant::now();
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
            "--timeout-ms",
            "2000",
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
    assert!(started.elapsed().as_millis() <= 2_000);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["timed_out"], false);
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
