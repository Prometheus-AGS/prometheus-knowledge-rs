use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn mechanical_json_lint_never_requires_a_model() {
    let fixture = tempfile::tempdir().unwrap();
    let wiki = fixture.path().join("wiki");
    let caller = fixture.path().join("caller");
    fs::create_dir_all(&wiki).unwrap();
    fs::create_dir_all(caller.join(".git")).unwrap();
    fs::write(
        wiki.join("valid.md"),
        "---\ntype: Reference\ntitle: Valid\ndescription: A valid page.\n---\n\nUseful body.\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&caller)
        .env("PK_LINT_URL", "http://127.0.0.1:1")
        .args([
            "--kb-dir",
            fixture.path().to_str().unwrap(),
            "lint",
            "--mechanical-only",
            "--json",
            "--strict-errors",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["issues"].as_array().unwrap().len(), 0);
    assert!(!caller.join(".prometheus").exists());
}

#[test]
fn strict_mechanical_lint_fails_for_unparseable_frontmatter() {
    let fixture = tempfile::tempdir().unwrap();
    let wiki = fixture.path().join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    fs::write(
        wiki.join("broken.md"),
        "---\ntitle: unsafe: unquoted\n---\n\nBody.\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .args([
            "--kb-dir",
            fixture.path().to_str().unwrap(),
            "lint",
            "--mechanical-only",
            "--json",
            "--strict-errors",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["severity"] == "error"));
}
