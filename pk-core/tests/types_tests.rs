use pk_core::event::LibrarianEvent;
use pk_core::types::{ArticleId, LintReport, LintSeverity, RawDoc, WikiEntry};

#[test]
fn article_id_slug_from_title() {
    let id = ArticleId::from_slug("Universal Agent Runtime");
    assert_eq!(id.as_str(), "universal-agent-runtime");
}

#[test]
fn article_id_slug_collapses_spaces_and_punctuation() {
    let id = ArticleId::from_slug("  SurrealDB  +  Rust!!  ");
    assert_eq!(id.as_str(), "surrealdb-rust");
}

#[test]
fn article_id_display() {
    let id = ArticleId::from("prometheus-mesh");
    assert_eq!(id.to_string(), "prometheus-mesh");
}

#[test]
fn wiki_entry_builder_roundtrip() {
    let entry = WikiEntry::new(
        "TurboQuant KV Compression",
        "3-bit FWHT KV cache compression.",
    )
    .with_tags(["rust", "turboquant", "kv-cache"])
    .with_sources(["session:2026-04-10"]);

    assert_eq!(entry.title, "TurboQuant KV Compression");
    assert_eq!(entry.id, ArticleId::from_slug("TurboQuant KV Compression"));
    assert_eq!(entry.tags, vec!["rust", "turboquant", "kv-cache"]);
    assert_eq!(entry.sources, vec!["session:2026-04-10"]);
    assert_eq!(entry.revision, 0);
    assert!(entry.links.is_empty());
}

#[test]
fn wiki_entry_bump_revision_increments() {
    let mut entry = WikiEntry::new("Test", "body");
    assert_eq!(entry.revision, 0);
    entry.bump_revision();
    assert_eq!(entry.revision, 1);
    entry.bump_revision();
    assert_eq!(entry.revision, 2);
}

#[test]
fn wiki_entry_serde_roundtrip() {
    let original = WikiEntry::new(
        "Kaia Agent Certification",
        "W3C Verifiable Credentials for agents.",
    )
    .with_tags(["kaia", "vc", "did"])
    .with_sources(["kaia-mvp-spec.md"]);

    let json = serde_json::to_string(&original).unwrap();
    let recovered: WikiEntry = serde_json::from_str(&json).unwrap();

    assert_eq!(recovered.id, original.id);
    assert_eq!(recovered.title, original.title);
    assert_eq!(recovered.tags, original.tags);
    assert_eq!(recovered.revision, original.revision);
}

#[test]
fn raw_doc_media_type_inferred() {
    use pk_core::types::RawDocMediaType;

    let md = RawDoc::from_path("notes.md", "content");
    let txt = RawDoc::from_path("notes.txt", "content");
    let js = RawDoc::from_path("data.json", "{}");

    assert_eq!(md.media_type, RawDocMediaType::Markdown);
    assert_eq!(txt.media_type, RawDocMediaType::PlainText);
    assert_eq!(js.media_type, RawDocMediaType::Json);
}

#[test]
fn lint_severity_ordering() {
    assert!(LintSeverity::Error > LintSeverity::Warning);
    assert!(LintSeverity::Warning > LintSeverity::Info);
}

#[test]
fn lint_report_serde() {
    let report = LintReport {
        entry_id: Some(ArticleId::from("universal-agent-runtime")),
        severity: LintSeverity::Warning,
        issue: "Missing link to mempalace-rs".to_owned(),
        suggestion: "Add 'mempalace-rs' to the links field".to_owned(),
        auto_fixable: true,
    };

    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("\"warning\""));
    assert!(json.contains("universal-agent-runtime"));

    let recovered: LintReport = serde_json::from_str(&json).unwrap();
    assert_eq!(recovered.severity, LintSeverity::Warning);
    assert!(recovered.auto_fixable);
}

#[test]
fn librarian_event_compiled_serde() {
    let event = LibrarianEvent::compiled(
        ArticleId::from("uar"),
        "Universal Agent Runtime".into(),
        vec!["rust".into(), "uar".into()],
    );

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"compiled\""));
}

#[test]
fn librarian_event_error_serde() {
    let event = LibrarianEvent::error("LLM timeout after 180s");
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"error\""));
    assert!(json.contains("LLM timeout"));
}
