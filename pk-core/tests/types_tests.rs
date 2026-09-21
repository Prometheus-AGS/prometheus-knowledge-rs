use pk_core::event::LibrarianEvent;
use pk_core::types::{ArticleId, LintReport, LintSeverity, RawDoc, Source, WikiEntry};

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
fn a_nested_concept_id_is_a_safe_path() {
    assert!(ArticleId::from("tables/orders").is_safe_path());
    assert!(ArticleId::from("v1.2-release-notes").is_safe_path());
}

#[test]
fn a_posix_traversal_or_absolute_id_is_not_a_safe_path() {
    assert!(!ArticleId::from("../evil").is_safe_path());
    assert!(!ArticleId::from("/etc/passwd").is_safe_path());
    assert!(!ArticleId::from("a//b").is_safe_path());
}

// An id is joined onto the wiki root as a filesystem path, and it arrives from
// ingested frontmatter and model output. On Windows a backslash is a separator
// and a colon introduces a drive, so both escape the root there while looking
// like an ordinary file name on unix.
#[test]
fn a_windows_traversal_or_drive_id_is_not_a_safe_path() {
    assert!(!ArticleId::from(r"..\..\evil").is_safe_path());
    assert!(!ArticleId::from(r"\evil").is_safe_path());
    assert!(!ArticleId::from(r"tables\orders").is_safe_path());
    assert!(!ArticleId::from("C:/evil").is_safe_path());
    assert!(!ArticleId::from("C:evil").is_safe_path());
}

// wiki\con.md opens the console device: the write reports success and no file
// exists. Windows also strips a trailing dot or space, so "a." and "a" collide.
#[test]
fn a_windows_device_name_or_trailing_dot_is_not_a_safe_path() {
    assert!(!ArticleId::from("con").is_safe_path());
    assert!(!ArticleId::from("NUL").is_safe_path());
    assert!(!ArticleId::from("tables/com1").is_safe_path());
    assert!(!ArticleId::from("aux.notes").is_safe_path());
    assert!(!ArticleId::from("lpt9/orders").is_safe_path());
    assert!(!ArticleId::from("orders.").is_safe_path());
    assert!(!ArticleId::from("orders ").is_safe_path());
}

#[test]
fn a_name_that_merely_starts_like_a_device_is_a_safe_path() {
    assert!(ArticleId::from("console").is_safe_path());
    assert!(ArticleId::from("com10").is_safe_path());
    assert!(ArticleId::from("nullable-columns").is_safe_path());
}

#[test]
fn a_title_that_slugs_to_a_device_name_still_yields_a_safe_id() {
    let id = ArticleId::from_slug("Con");

    assert!(id.is_safe_path());
    assert_ne!(id, ArticleId::from_slug("Con Entry"));
}

// OKF v0.2 §5.1: a source is a mapping with a required `resource`. pk used to
// hold sources as strings; a string cannot carry the per-source signals v0.2
// defines, so reading one and writing it back would silently drop them.
#[test]
fn a_bare_string_source_becomes_a_resource() {
    let from_yaml: Vec<Source> = serde_yaml::from_str("- session:abc-123\n").unwrap();
    let from_json: Vec<Source> = serde_json::from_str(r#"["session:abc-123"]"#).unwrap();

    assert_eq!(from_yaml[0].resource, "session:abc-123");
    assert_eq!(from_yaml[0].id, None);
    assert_eq!(from_json, from_yaml);
}

#[test]
fn a_mapping_source_keeps_every_key_through_a_round_trip() {
    let yaml = "- id: ga4-schema\n  resource: https://example.com/schema\n  title: GA4 schema\n  author: team:ga4-docs\n  usage_count: 5000\n  last_modified: 2026-05-30T00:00:00Z\n";

    let sources: Vec<Source> = serde_yaml::from_str(yaml).unwrap();
    let written = serde_yaml::to_string(&sources).unwrap();
    let reread: Vec<Source> = serde_yaml::from_str(&written).unwrap();

    assert_eq!(sources[0].id.as_deref(), Some("ga4-schema"));
    assert_eq!(sources[0].resource, "https://example.com/schema");
    for key in ["title", "author", "usage_count", "last_modified"] {
        assert!(sources[0].extra.contains_key(key), "lost {key} on read");
        assert!(written.contains(key), "lost {key} on write: {written}");
    }
    assert_eq!(reread, sources);
}

#[test]
fn a_source_is_written_as_a_mapping_never_as_a_string() {
    let written = serde_yaml::to_string(&vec![Source::from("session:abc-123")]).unwrap();

    assert_eq!(written.trim(), "- resource: session:abc-123");
}

#[test]
fn a_mapping_source_without_a_resource_names_the_missing_key() {
    let error = serde_yaml::from_str::<Vec<Source>>("- id: orphan\n  title: no resource\n")
        .unwrap_err()
        .to_string();

    assert!(error.contains("resource"), "{error}");
}

#[test]
fn with_sources_still_accepts_strings() {
    let entry = WikiEntry::new("T", "body").with_sources(["session:abc-123"]);

    assert_eq!(entry.sources, vec![Source::from("session:abc-123")]);
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
