use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path, process::Command};

fn collect_paths(root: &Path) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            paths.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    paths
}

#[test]
fn doctor_is_non_mutating_and_reports_current_runtime_surfaces() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    let knowledge = project.join(".prometheus/knowledge");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&knowledge).unwrap();
    fs::create_dir_all(project.join(".git")).unwrap();
    let before_home = collect_paths(&home);
    let before_project = collect_paths(&project);

    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("RUST_LOG", "error")
        .args([
            "--kb-dir",
            knowledge.to_str().unwrap(),
            "doctor",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success(), "empty fixture should be unhealthy");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["checks"].as_array().unwrap();
    let names = checks
        .iter()
        .filter_map(|check| check["name"].as_str())
        .collect::<BTreeSet<_>>();
    for expected in [
        "hooks-log-path",
        "plugin-generation",
        "stable-dispatchers",
        "prompt-snapshots",
        "learning-queue",
        "kb-scoping",
    ] {
        assert!(names.contains(expected), "missing {expected}: {report}");
    }
    assert_eq!(before_home, collect_paths(&home));
    assert_eq!(before_project, collect_paths(&project));
}
