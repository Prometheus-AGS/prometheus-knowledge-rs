//! `pk ingest --type/--tag` classifies entries and `pk context --tag` filters on
//! those tags, so recall can target a role, team or visibility level.
//!
//! The compile step calls a model; the test points pk at a local
//! OpenAI-compatible endpoint that returns a fixed compile result per request,
//! so the real CLI, librarian, store and snapshot path run end to end.

use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

/// Serves `/v1/chat/completions`, answering request N with compile JSON whose
/// title and content are `titles[N]`.
fn fake_model(titles: Vec<&'static str>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = Arc::new(AtomicUsize::new(0));
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 16384];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&request);
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text[..split]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if request.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            let index = served.fetch_add(1, Ordering::SeqCst);
            let title = titles[index.min(titles.len() - 1)];
            let compiled = json!({
                "title": title,
                "content": format!("{title} body with tagfiltertoken"),
                "tags": ["model-tag"]
            });
            let body =
                json!({"choices": [{"message": {"content": compiled.to_string()}}]}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            if index + 1 >= titles.len() {
                break;
            }
        }
    });
    (format!("http://{address}/v1"), handle)
}

fn pk(project: &Path, home: &Path, model_url: &str, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pk"))
        .current_dir(project)
        .env("HOME", home)
        .env("RUST_LOG", "error")
        .env("CLOUD_LLM_URL", model_url)
        .env("CLOUD_LLM_API_KEY", "test-key")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "pk {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn entry_frontmatter(project: &Path, title_slug_prefix: &str) -> String {
    let wiki = project.join(".prometheus/knowledge/wiki");
    let file = fs::read_dir(&wiki)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(title_slug_prefix))
        })
        .unwrap_or_else(|| panic!("no wiki entry starting {title_slug_prefix} in {wiki:?}"));
    let text = fs::read_to_string(file).unwrap();
    text.split("---").nth(1).unwrap_or_default().to_owned()
}

#[test]
fn ingest_sets_type_and_tags_and_context_filters_by_tag() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let project = fixture.path().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::create_dir_all(&home).unwrap();
    let (url, server) = fake_model(vec!["Api role lesson", "Ui role lesson", "Untyped note"]);

    pk(
        &project,
        &home,
        &url,
        &["ingest", "--type", "Lesson", "--tag", "role:api-dev"],
        Some("api lesson source\n"),
    );
    pk(
        &project,
        &home,
        &url,
        &[
            "ingest",
            "--type",
            "Gotcha",
            "--tag",
            "role:ui-dev",
            "--tag",
            "vis:team",
        ],
        Some("ui lesson source\n"),
    );
    pk(
        &project,
        &home,
        &url,
        &["ingest"],
        Some("untyped note source\n"),
    );
    server.join().unwrap();

    let api = entry_frontmatter(&project, "api-role-lesson");
    assert!(api.contains("type: Lesson"), "{api}");
    assert!(
        api.contains("role:api-dev") && api.contains("model-tag"),
        "{api}"
    );
    let ui = entry_frontmatter(&project, "ui-role-lesson");
    assert!(ui.contains("type: Gotcha"), "{ui}");
    assert!(
        ui.contains("role:ui-dev") && ui.contains("vis:team"),
        "{ui}"
    );
    let untyped = entry_frontmatter(&project, "untyped-note");
    assert!(
        untyped.contains("type: Reference"),
        "default type must stay Reference: {untyped}"
    );

    let context = |tags: &[&str]| -> Value {
        let mut args = vec![
            "context",
            "tagfiltertoken",
            "--scope",
            "project",
            "--format",
            "json",
        ];
        for tag in tags {
            args.push("--tag");
            args.push(tag);
        }
        serde_json::from_slice(&pk(&project, &home, &url, &args, None).stdout).unwrap()
    };
    let ids = |report: &Value| -> Vec<String> {
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect()
    };

    let api_only = context(&["role:api-dev"]);
    assert_eq!(ids(&api_only), ["api-role-lesson"], "{api_only:#}");

    let both_tags = context(&["role:ui-dev", "vis:team"]);
    assert_eq!(ids(&both_tags), ["ui-role-lesson"], "{both_tags:#}");

    let no_match = context(&["role:api-dev", "vis:team"]);
    assert!(ids(&no_match).is_empty(), "tags are all-of: {no_match:#}");

    let unfiltered = context(&[]);
    assert_eq!(ids(&unfiltered).len(), 3, "{unfiltered:#}");
}
