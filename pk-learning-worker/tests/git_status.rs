//! Real worker/Git regression coverage. Set PK_GIT_STATUS_WORKER_BIN to run the
//! same scenarios against a preserved pre-change worker as a negative control.

use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant, SystemTime},
};

const EVENT_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Fixture {
    _directory: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    queue: PathBuf,
}

fn isolate(command: &mut Command, home: &Path) {
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("empty-gitconfig"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OPTIONAL_LOCKS");
}

fn git(home: &Path, project: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    isolate(&mut command, home);
    let output = command.current_dir(project).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn init_repository(home: &Path, project: &Path) {
    fs::create_dir_all(project).unwrap();
    git(home, project, &["init", "--quiet"]);
    git(
        home,
        project,
        &["config", "user.name", "Git status fixture"],
    );
    git(
        home,
        project,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(home, project, &["config", "commit.gpgsign", "false"]);
    git(home, project, &["config", "core.fsmonitor", "false"]);
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let project = directory.path().join("project");
        let queue = home.join(".prometheus/learning-queue");
        fs::create_dir_all(queue.join("pending")).unwrap();
        init_repository(&home, &project);
        Self {
            _directory: directory,
            home,
            project,
            queue,
        }
    }

    fn git(&self, args: &[&str]) -> Output {
        git(&self.home, &self.project, args)
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.project.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", "fixture"]);
    }

    fn run(&self) -> (String, String, Duration) {
        fs::write(
            self.queue.join("pending").join(format!("{EVENT_ID}.json")),
            serde_json::to_vec(&json!({
                "schemaVersion": 2,
                "eventId": EVENT_ID,
                "eventType": "stop",
                "harness": "fixture",
                "sessionId": "git-status-fixture",
                "projectRoot": self.project,
                "transcriptPath": null,
                "capturedAt": "2026-11-23T00:00:00Z",
                "payloadDigest": "fixture",
                "attempt": 0
            }))
            .unwrap(),
        )
        .unwrap();
        let binary = std::env::var_os("PK_GIT_STATUS_WORKER_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_prometheus-learning-worker").into());
        let mut command = Command::new(binary);
        isolate(&mut command, &self.home);
        let start = Instant::now();
        let output = command
            .current_dir(&self.project)
            .env("RUST_LOG", "warn")
            .arg("--queue-root")
            .arg(&self.queue)
            .args(["--memory-url", "http://127.0.0.1:1", "run-once"])
            .output()
            .unwrap();
        let elapsed = start.elapsed();
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{logs}");
        assert!(
            self.queue
                .join("completed")
                .join(format!("{EVENT_ID}.json"))
                .exists(),
            "job did not complete: {logs}"
        );
        let packet = fs::read_to_string(self.project.join(format!(
            ".prometheus/knowledge/wiki/karpathy-session-{}.md",
            &EVENT_ID[..16]
        )))
        .unwrap();
        let paths = packet
            .split_once("## Changed Paths\n\n")
            .unwrap()
            .1
            .to_owned();
        (paths, logs, elapsed)
    }
}

#[test]
fn reports_top_level_changes_and_excludes_its_own_knowledge() {
    let fixture = Fixture::new();
    for name in ["modified.txt", "staged.txt", "deleted.txt", "rename.txt"] {
        fixture.write(name, &format!("original {name}\n"));
    }
    fixture.write(".prometheus/knowledge/collector.txt", "old\n");
    fixture.commit();
    fixture.write("modified.txt", "modified\n");
    fixture.write("staged.txt", "staged\n");
    fixture.git(&["add", "staged.txt"]);
    fs::remove_file(fixture.project.join("deleted.txt")).unwrap();
    fixture.git(&["mv", "rename.txt", "renamed.txt"]);
    fixture.write("untracked.txt", "untracked\n");
    fixture.write(".prometheus/knowledge/collector.txt", "new\n");
    let (paths, logs, _) = fixture.run();
    for expected in [
        "- modified.txt",
        "- staged.txt",
        "- deleted.txt",
        "- rename.txt -> renamed.txt",
        "- untracked.txt",
    ] {
        assert!(paths.lines().any(|line| line == expected), "{paths}");
    }
    assert!(!paths.contains(".prometheus"), "{paths}");
    assert!(!logs.contains("git status timed out"), "{logs}");
}

#[test]
fn ignores_dirty_submodules() {
    let fixture = Fixture::new();
    let nested = fixture.home.join("submodule-source");
    init_repository(&fixture.home, &nested);
    fs::write(nested.join("nested.txt"), "original\n").unwrap();
    git(&fixture.home, &nested, &["add", "--all"]);
    git(
        &fixture.home,
        &nested,
        &["commit", "--quiet", "-m", "fixture"],
    );
    fixture.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        nested.to_str().unwrap(),
        "vendor",
    ]);
    fixture.commit();
    fixture.write("vendor/nested.txt", "dirty\n");
    fixture.write("visible.txt", "top level\n");
    let (paths, _, _) = fixture.run();
    assert!(paths.lines().any(|line| line == "- visible.txt"), "{paths}");
    assert!(!paths.contains("vendor"), "{paths}");
}

#[test]
fn limits_reported_paths_to_forty() {
    let fixture = Fixture::new();
    for number in 0..45 {
        fixture.write(&format!("path-{number:02}.txt"), "untracked\n");
    }
    let (paths, _, _) = fixture.run();
    let reported: Vec<_> = paths
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect();
    assert_eq!(reported.len(), 40, "{paths}");
    assert_eq!(reported[0], "- path-00.txt");
    assert_eq!(reported[39], "- path-39.txt");
}

#[test]
fn preserves_an_existing_index_lock() {
    let fixture = Fixture::new();
    fixture.write("tracked.txt", "original\n");
    fixture.commit();
    fixture.write("tracked.txt", "changed\n");
    let lock = fixture.project.join(".git/index.lock");
    let sentinel = b"another writer owns this lock\n";
    fs::write(&lock, sentinel).unwrap();
    let (paths, _, _) = fixture.run();
    assert_eq!(fs::read(lock).unwrap(), sentinel);
    assert!(paths.lines().any(|line| line == "- tracked.txt"), "{paths}");
}

#[test]
fn leaves_the_index_unchanged_when_stat_metadata_needs_refreshing() {
    let fixture = Fixture::new();
    fixture.write("tracked.txt", "unchanged content\n");
    fixture.commit();
    let index = fixture.project.join(".git/index");
    let before = fs::read(&index).unwrap();
    // A real metadata mismatch forces Git to inspect the content and makes an
    // ordinary status refresh its index, even though no content has changed.
    fs::File::options()
        .write(true)
        .open(fixture.project.join("tracked.txt"))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    let (paths, _, _) = fixture.run();
    assert!(paths.contains("No changed paths detected."), "{paths}");
    assert_eq!(fs::read(index).unwrap(), before, "status rewrote the index");
}

#[test]
fn times_out_real_git_without_leaving_a_lock_and_allows_the_next_write() {
    let fixture = Fixture::new();
    let names: Vec<_> = (0..6_000)
        .map(|number| format!("{number:05}-{}.txt", "x".repeat(190)))
        .collect();
    for name in &names {
        fixture.write(name, "original\n");
    }
    fixture.commit();
    for name in &names {
        fixture.write(name, "changed\n");
    }
    // Prove the real output exceeds 1 MiB. The worker waits before draining its
    // pipe, so Git cannot finish writing this output before the five-second kill.
    let status = fixture.git(&["--no-optional-locks", "status", "--short"]);
    assert!(status.stdout.len() > 1_048_576);
    let (paths, logs, elapsed) = fixture.run();
    assert!(elapsed >= Duration::from_secs(5), "elapsed: {elapsed:?}");
    assert!(
        logs.contains("git status timed out; changed paths unavailable"),
        "{logs}"
    );
    assert!(paths.contains("No changed paths detected."), "{paths}");
    assert!(!fixture.project.join(".git/index.lock").exists());
    fixture.git(&["add", "--", &names[0]]);
}
