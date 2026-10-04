//! `pk context` must score every committed snapshot entry before any candidate
//! budget applies. Before this was fixed, only the first
//! `ceil(max_candidates / scopes)` entries of each scope, in snapshot order,
//! were ever scored, so a lesson that sorted late was never recalled.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn write_entry(base: &Path, id: &str, body: &str) {
    let wiki = base.join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    fs::write(
        wiki.join(format!("{id}.md")),
        format!("---\ntype: Lesson\ntitle: {id}\ntags: [fixture]\n---\n\n{body}\n"),
    )
    .unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

fn fixture(project_has_root: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    if project_has_root {
        fs::create_dir_all(project.join(".git")).unwrap();
    }
    fs::create_dir_all(&home).unwrap();
    Fixture {
        _dir: dir,
        home,
        project,
    }
}

fn pk(f: &Fixture, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(&f.project)
        .env("HOME", &f.home)
        .env("RUST_LOG", "error")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pk {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn an_entry_sorting_last_in_a_large_scope_is_still_recalled_with_default_flags() {
    let f = fixture(true);
    let kb = f.project.join(".prometheus/knowledge");
    for index in 0..199 {
        write_entry(
            &kb,
            &format!("entry-{index:03}"),
            "unrelated filler lesson about build caching",
        );
    }
    // Sorts after every filler entry by id.
    write_entry(
        &kb,
        "zzz-target",
        "latesortingtoken lesson that must be recalled",
    );
    pk(&f, &["snapshot", "--scope", "project"]);

    let report = json(&pk(
        &f,
        &[
            "context",
            "latesortingtoken",
            "--scope",
            "project",
            "--format",
            "json",
        ],
    ));
    assert_eq!(report["scored_count"], 200, "{report:#}");
    assert_eq!(report["results"][0]["id"], "zzz-target", "{report:#}");
}

#[test]
fn a_failed_scope_does_not_reserve_budget_the_remaining_scopes_can_use() {
    // No project root: the project scope fails, and the shared scope must be
    // able to fill the whole cap on its own.
    let f = fixture(false);
    let shared = f.home.join(".prometheus/knowledge/shared");
    for index in 0..60 {
        write_entry(
            &shared,
            &format!("shared-{index:02}"),
            "budgettoken shared lesson",
        );
    }
    pk(&f, &["snapshot", "--scope", "shared"]);

    let report = json(&pk(
        &f,
        &[
            "context",
            "budgettoken",
            "--scope",
            "project",
            "--scope",
            "shared",
            "--max-candidates",
            "60",
            "--limit",
            "32",
            "--format",
            "json",
        ],
    ));
    assert_eq!(
        report["failures"].as_array().unwrap().len(),
        1,
        "{report:#}"
    );
    assert_eq!(report["candidate_count"], 60, "{report:#}");
    assert_eq!(
        report["results"].as_array().unwrap().len(),
        32,
        "{report:#}"
    );
}

#[test]
fn output_is_deterministic_and_ranked_by_score_then_scope_then_id() {
    let f = fixture(true);
    let project_kb = f.project.join(".prometheus/knowledge");
    let shared = f.home.join(".prometheus/knowledge/shared");
    // Equal scores across scopes: project must precede shared, then id order.
    write_entry(&project_kb, "b-project", "ordertoken lesson");
    write_entry(&project_kb, "a-project", "ordertoken lesson");
    write_entry(&shared, "a-shared", "ordertoken different lesson");
    // Higher score (token appears twice) ranks first regardless of scope.
    write_entry(&shared, "z-strong", "ordertoken ordertoken strong lesson");
    pk(&f, &["snapshot", "--scope", "project", "--scope", "shared"]);

    let args = [
        "context",
        "ordertoken",
        "--scope",
        "project",
        "--scope",
        "shared",
        "--format",
        "json",
    ];
    let first = pk(&f, &args).stdout;
    let second = pk(&f, &args).stdout;
    assert_eq!(
        first, second,
        "context output must be byte-identical across runs"
    );

    let report: Value = serde_json::from_slice(&first).unwrap();
    let results = report["results"].as_array().unwrap();
    let scores: Vec<f64> = results
        .iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    assert!(
        scores.windows(2).all(|w| w[0] >= w[1]),
        "results must be ordered by descending score: {report:#}"
    );
    // Among entries with the top-but-one score, project entries come first in id order.
    let tied: Vec<(&str, &str)> = results
        .iter()
        .filter(|r| r["score"].as_f64().unwrap() == scores[scores.len() - 1])
        .map(|r| (r["scope"].as_str().unwrap(), r["id"].as_str().unwrap()))
        .collect();
    let mut expected = tied.clone();
    expected.sort_by(|left, right| {
        let rank = |scope: &str| if scope == "project" { 0 } else { 1 };
        rank(left.0).cmp(&rank(right.0)).then(left.1.cmp(right.1))
    });
    assert_eq!(tied, expected, "{report:#}");
}
