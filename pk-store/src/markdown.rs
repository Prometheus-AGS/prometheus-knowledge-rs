use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, Source, WikiEntry},
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

/// Filenames reserved by the Open Knowledge Format (OKF v0.2 §3.1) at any
/// level of a bundle. Never treated as concept documents.
pub const RESERVED_FILENAMES: [&str; 2] = ["index.md", "log.md"];

pub fn is_reserved_filename(name: &str) -> bool {
    RESERVED_FILENAMES.contains(&name)
}

/// OKF v0.2 §5.2 `generated`: how the current content was produced. `by` is
/// an actor (§7) and is required within the mapping; `at` marks the last
/// meaningful change. The spec defines only these two keys; pk carries `by` on the
/// entry and derives `at`, so any other key inside `generated` is not preserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Generated {
    by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<String>,
}

/// Read `generated` leniently. pk 1.8.0 did not model the key, so a document
/// carrying any shape of it parsed; a strict type here would turn a document
/// that used to load into one that fails, and a parse failure is not local —
/// it freezes live reload and blocks snapshot commits for the whole store. So
/// anything but a mapping with a string `by` (§5.2 requires `by`) is absent,
/// and an `at` that is not a string is ignored.
fn lenient_generated<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Generated>, D::Error> {
    let value = Option::<serde_yaml::Value>::deserialize(deserializer)?;
    let Some(serde_yaml::Value::Mapping(mapping)) = value else {
        return Ok(None);
    };
    let text = |key: &str| mapping.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Ok(text("by").map(|by| Generated { by, at: text("at") }))
}

/// The actor pk names when a document carried no `generated.by` of its own:
/// `<producer>/<version>`, the §7 form for agents and tools.
const PK_ACTOR: &str = concat!("pk/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// Frontmatter — permissive per OKF v0.2 §4.1 and §11. `type` is OKF's one
// required key; every pk-native field is optional so both a minimal OKF
// document and a legacy pre-OKF pk document parse without error. Keys this
// struct doesn't model are captured in `extra` and preserved verbatim on
// round-trip (OKF v0.2 §4.1: consumers SHOULD preserve unknown keys).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Frontmatter {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    entry_type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<Source>,
    /// OKF v0.2 §5.2. Typed rather than left to `extra`, so it is never written
    /// twice.
    #[serde(
        default,
        deserialize_with = "lenient_generated",
        skip_serializing_if = "Option::is_none"
    )]
    generated: Option<Generated>,
    /// OKF v0.1 §4.1 `timestamp`, superseded by `generated.at` (v0.2 §13.1).
    /// Read as a fallback for v0.1 documents; never written.
    #[serde(default, skip_serializing)]
    timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<u32>,

    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

pub fn entry_to_markdown(entry: &WikiEntry) -> PkResult<String> {
    let fm = Frontmatter {
        entry_type: entry.entry_type.clone(),
        id: Some(entry.id.as_str().to_owned()),
        title: Some(entry.title.clone()),
        description: entry.description.clone(),
        resource: None,
        tags: entry.tags.clone(),
        links: entry.links.iter().map(|l| l.as_str().to_owned()).collect(),
        sources: entry.sources.clone(),
        // `by` is only defaulted: a model or a person may be the author, and
        // stamping pk over them would erase that. `at` always tracks
        // `updated_at`, which is also written below as a pk extension key.
        generated: Some(Generated {
            by: entry
                .generated_by
                .clone()
                .unwrap_or_else(|| PK_ACTOR.to_owned()),
            at: Some(entry.updated_at.to_rfc3339()),
        }),
        timestamp: None,
        created_at: Some(entry.created_at.to_rfc3339()),
        updated_at: Some(entry.updated_at.to_rfc3339()),
        revision: Some(entry.revision),
        extra: entry.extra.clone(),
    };

    let yaml = serde_yaml::to_string(&fm).map_err(|e| PkError::frontmatter(e.to_string()))?;

    Ok(format!("---\n{}---\n\n{}", yaml, entry.content))
}

/// Parse a markdown document into a `WikiEntry`.
///
/// `fallback_id` supplies the concept ID when frontmatter omits `id` (as any
/// conformant OKF document may) — callers with a file path pass the
/// wiki-relative path (minus `.md`) per OKF v0.2 §2's Concept ID definition.
pub fn markdown_to_entry(raw: &str, fallback_id: Option<&str>) -> PkResult<WikiEntry> {
    // Accept CRLF, keep LF: the body feeds the content hash and the snapshot
    // generation id, which must not depend on the checkout that produced it.
    let raw: Cow<str> = if raw.contains('\r') {
        Cow::Owned(raw.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(raw)
    };
    let raw = raw.trim_start();

    if !raw.starts_with("---") {
        return Err(PkError::frontmatter("missing frontmatter fence"));
    }

    let rest = &raw[3..];
    let end = rest
        .find("\n---")
        .ok_or_else(|| PkError::frontmatter("unclosed frontmatter fence"))?;

    let yaml_str = &rest[..end];
    let body_start = end + 4;
    let content = rest
        .get(body_start..)
        .unwrap_or("")
        .trim_start_matches('\n')
        .to_owned();

    let fm: Frontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| PkError::frontmatter(format!("yaml parse: {e}")))?;

    let id = fm
        .id
        .or_else(|| fallback_id.map(str::to_owned))
        .ok_or_else(|| PkError::frontmatter("no id in frontmatter and no fallback path given"))?;

    if !ArticleId::from(id.clone()).is_safe_path() {
        return Err(PkError::frontmatter(format!(
            "id {id:?} is not a safe concept path (no leading '/', no '\\' or ':', and no segment that is empty, '..', a Windows device name, or ends in '.' or a space)"
        )));
    }

    let title = fm.title.unwrap_or_else(|| id.clone());

    let now = chrono::Utc::now();
    let created_at = match fm.created_at {
        Some(ref s) => chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|e| PkError::frontmatter(format!("created_at: {e}")))?
            .with_timezone(&chrono::Utc),
        None => now,
    };
    // An explicit `updated_at` (pk-native) wins, then v0.2's `generated.at`,
    // then v0.1's `timestamp` (§13.1 allows the fallback). `updated_at` and
    // `timestamp` stay as strict as 1.8.0 had them. `generated.at` is new to pk:
    // chrono's RFC 3339 parser is narrower than "ISO 8601 with an offset", and a
    // value it rejects falls through rather than failing a document 1.8.0 read.
    let strict = |field: &str, s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|at| at.with_timezone(&chrono::Utc))
            .map_err(|e| PkError::frontmatter(format!("{field}: {e}")))
    };
    let generated_at = fm
        .generated
        .as_ref()
        .and_then(|g| g.at.as_deref())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|at| at.with_timezone(&chrono::Utc));
    let updated_at = match (
        fm.updated_at.as_deref(),
        generated_at,
        fm.timestamp.as_deref(),
    ) {
        (Some(s), _, _) => strict("updated_at", s)?,
        (None, Some(at), _) => at,
        (None, None, Some(s)) => strict("timestamp", s)?,
        (None, None, None) => now,
    };

    Ok(WikiEntry {
        id: ArticleId::from(id),
        title,
        content,
        tags: fm.tags,
        links: fm.links.into_iter().map(ArticleId::from).collect(),
        sources: fm.sources,
        created_at,
        updated_at,
        revision: fm.revision.unwrap_or(1),
        entry_type: fm.entry_type,
        description: fm.description,
        generated_by: fm.generated.map(|g| g.by),
        extra: fm.extra,
    })
}

pub fn article_filename(id: &ArticleId) -> String {
    format!("{}.md", id.as_str())
}

pub fn article_path(base: &Path, id: &ArticleId) -> std::path::PathBuf {
    base.join(article_filename(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_entry() {
        let entry = WikiEntry::new(
            "Universal Agent Runtime",
            "The UAR is the core of Prometheus.",
        )
        .with_tags(["rust", "uar", "prometheus"])
        .with_sources(["session:abc-123"]);

        let md = entry_to_markdown(&entry).unwrap();
        assert!(md.contains("---"));
        assert!(md.contains("Universal Agent Runtime"));

        let recovered = markdown_to_entry(&md, None).unwrap();
        assert_eq!(recovered.id, entry.id);
        assert_eq!(recovered.title, entry.title);
        assert_eq!(recovered.content, entry.content);
        assert_eq!(recovered.tags, entry.tags);
        assert_eq!(recovered.revision, entry.revision);
    }

    // A Windows checkout (or editor) hands the parser CRLF. The body must not
    // keep the carriage returns: it feeds the content hash and the snapshot
    // generation id, so the same KB would hash differently per OS.
    #[test]
    fn a_crlf_document_parses_identically_to_its_lf_twin() {
        let lf = "---\nid: tables/orders\ntitle: Orders\ntags: [sql, core]\n---\n\nFirst line.\nSecond line.\n";
        let crlf = lf.replace('\n', "\r\n");

        let from_lf = markdown_to_entry(lf, None).unwrap();
        let from_crlf = markdown_to_entry(&crlf, None).unwrap();

        assert_eq!(from_crlf.content, "First line.\nSecond line.\n");
        assert_eq!(from_crlf.content, from_lf.content);
        assert_eq!(from_crlf.id, from_lf.id);
        assert_eq!(from_crlf.title, from_lf.title);
        assert_eq!(from_crlf.tags, from_lf.tags);
    }

    fn frontmatter_of(markdown: &str) -> serde_yaml::Mapping {
        let rest = markdown.strip_prefix("---\n").expect("opening fence");
        let end = rest.find("\n---").expect("closing fence");
        serde_yaml::from_str(&rest[..end]).expect("frontmatter parses, with no duplicate key")
    }

    // OKF v0.2 §13.1: `timestamp` is superseded by `generated: { by, at }`.
    #[test]
    fn a_written_entry_carries_generated_and_no_timestamp() {
        let entry = WikiEntry::new("Orders", "body");

        let fm = frontmatter_of(&entry_to_markdown(&entry).unwrap());

        let generated = fm
            .get("generated")
            .expect("generated")
            .as_mapping()
            .unwrap();
        let by = generated.get("by").unwrap().as_str().unwrap();
        assert!(by.starts_with("pk/"), "{by}");
        assert_eq!(
            generated.get("at").unwrap().as_str().unwrap(),
            entry.updated_at.to_rfc3339()
        );
        assert!(!fm.contains_key("timestamp"), "timestamp is superseded");
    }

    // pk is not always the author: a model compiled it, or a person edited it.
    #[test]
    fn a_producers_own_generated_by_survives_a_write() {
        let doc = "---\ntype: Reference\ngenerated: { by: librarian/some-model, at: 2026-06-20T22:53:05Z }\n---\n\nbody\n";

        let entry = markdown_to_entry(doc, Some("orders")).unwrap();
        let fm = frontmatter_of(&entry_to_markdown(&entry).unwrap());

        assert_eq!(entry.generated_by.as_deref(), Some("librarian/some-model"));
        let generated = fm.get("generated").unwrap().as_mapping().unwrap();
        assert_eq!(
            generated.get("by").unwrap().as_str(),
            Some("librarian/some-model")
        );
    }

    #[test]
    fn a_v01_timestamp_alone_still_sets_updated_at() {
        let doc = "---\ntype: Reference\ntimestamp: 2026-05-01T10:00:00Z\n---\n\nbody\n";

        let entry = markdown_to_entry(doc, Some("orders")).unwrap();

        assert_eq!(entry.updated_at.to_rfc3339(), "2026-05-01T10:00:00+00:00");
    }

    #[test]
    fn generated_at_outranks_an_older_timestamp() {
        let doc = "---\ntype: Reference\ntimestamp: 2026-05-01T10:00:00Z\ngenerated: { by: pk/1.0.0, at: 2026-07-01T10:00:00Z }\n---\n\nbody\n";

        let entry = markdown_to_entry(doc, Some("orders")).unwrap();

        assert_eq!(entry.updated_at.to_rfc3339(), "2026-07-01T10:00:00+00:00");
    }

    // `extra` is a flattened catch-all; a typed `generated` beside it must not
    // leave the key in both places.
    #[test]
    fn a_document_that_already_carries_generated_does_not_emit_it_twice() {
        let doc = "---\ntype: Reference\ngenerated: { by: pk/1.0.0, at: 2026-07-01T10:00:00Z }\n---\n\nbody\n";

        let entry = markdown_to_entry(doc, Some("orders")).unwrap();
        let written = entry_to_markdown(&entry).unwrap();

        assert!(!entry.extra.contains_key("generated"));
        assert_eq!(written.matches("\ngenerated:").count(), 1, "{written}");
        frontmatter_of(&written);
    }

    #[test]
    fn a_crlf_v02_document_has_the_same_provenance_as_its_lf_twin() {
        let lf = "---\ntype: Reference\ngenerated: { by: pk/1.0.0, at: 2026-07-01T10:00:00Z }\nsources:\n  - id: ga4\n    resource: https://example.com/schema\n    title: GA4 schema\n---\n\nbody\n";
        let crlf = lf.replace('\n', "\r\n");

        let from_lf = markdown_to_entry(lf, Some("orders")).unwrap();
        let from_crlf = markdown_to_entry(&crlf, Some("orders")).unwrap();

        assert_eq!(from_crlf.sources, from_lf.sources);
        assert_eq!(from_crlf.updated_at, from_lf.updated_at);
        assert_eq!(from_crlf.sources[0].id.as_deref(), Some("ga4"));
    }

    // pk 1.8.0 did not model `generated`: the key fell into `extra`, so a document
    // carrying any shape of it parsed. Typing the key must not turn a document
    // that used to load into one that fails — a parse failure also freezes live
    // reload and blocks snapshot commits for the whole store.
    #[test]
    fn a_generated_that_is_not_a_usable_mapping_is_treated_as_absent() {
        for generated in [
            "generated: written by hand",
            "generated: true",
            "generated: { at: 2026-07-01T10:00:00Z }",
            "generated: [pk, 2026]",
            "generated: { by: 7 }",
        ] {
            let doc = format!("---\ntype: Reference\n{generated}\n---\n\nbody\n");

            let entry = markdown_to_entry(&doc, Some("orders"))
                .unwrap_or_else(|e| panic!("{generated:?} must still parse: {e}"));

            assert_eq!(entry.generated_by, None, "{generated}");
            assert_eq!(entry.content, "body\n");
        }
    }

    // chrono's RFC 3339 parser is narrower than "ISO 8601 with an offset". 1.8.0
    // never looked at `generated.at`, so a value it cannot parse falls through to
    // the next source of `updated_at` instead of failing the document.
    #[test]
    fn an_unparseable_generated_at_falls_through_to_timestamp() {
        for at in [
            "2026-07-01T10:00:00",
            "2026-07-01",
            "2026-07-01T10:00+00:00",
            "not a date",
        ] {
            let doc = format!(
                "---\ntype: Reference\ntimestamp: 2026-05-01T10:00:00Z\ngenerated: {{ by: someone/1.0, at: \"{at}\" }}\n---\n\nbody\n"
            );

            let entry = markdown_to_entry(&doc, Some("orders"))
                .unwrap_or_else(|e| panic!("at={at:?} must still parse: {e}"));

            assert_eq!(
                entry.updated_at.to_rfc3339(),
                "2026-05-01T10:00:00+00:00",
                "{at}"
            );
            assert_eq!(entry.generated_by.as_deref(), Some("someone/1.0"), "{at}");
        }
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let result = markdown_to_entry("# No frontmatter here\n\nJust a body.", None);
        assert!(result.is_err());
    }

    /// OKF v0.2 §11: a bundle is conformant if every frontmatter block has a
    /// non-empty `type` — nothing else is required. This is the minimal
    /// legal OKF document; pk must parse it without an `id`.
    #[test]
    fn parses_minimal_okf_document() {
        let doc = "---\ntype: Reference\n---\n\nJust a body.";
        let entry = markdown_to_entry(doc, Some("some/concept")).unwrap();
        assert_eq!(entry.entry_type.as_deref(), Some("Reference"));
        assert_eq!(entry.id.as_str(), "some/concept");
        assert_eq!(entry.content, "Just a body.");
        // No id/title in frontmatter and no revision: pk's own defaults apply.
        assert_eq!(entry.revision, 1);
    }

    #[test]
    fn minimal_okf_document_without_fallback_id_is_an_error() {
        let doc = "---\ntype: Reference\n---\n\nJust a body.";
        assert!(markdown_to_entry(doc, None).is_err());
    }

    /// Back-compat: a pre-OKF pk document (no `type`, all pk-native fields
    /// present) must still parse exactly as it did before this change.
    #[test]
    fn parses_legacy_pk_document() {
        let doc = "---\nid: legacy-entry\ntitle: Legacy Entry\ntags:\n  - old\nlinks: []\nsources:\n  - session:abc\ncreated_at: \"2026-01-01T00:00:00Z\"\nupdated_at: \"2026-01-02T00:00:00Z\"\nrevision: 3\n---\n\nLegacy body.";
        let entry = markdown_to_entry(doc, None).unwrap();
        assert_eq!(entry.entry_type, None);
        assert_eq!(entry.id.as_str(), "legacy-entry");
        assert_eq!(entry.title, "Legacy Entry");
        assert_eq!(entry.revision, 3);
        assert_eq!(entry.tags, vec!["old".to_string()]);
    }

    /// OKF v0.2 §4.1: unknown frontmatter keys must survive a round-trip, not be
    /// silently dropped when pk re-serializes an entry it doesn't fully model.
    #[test]
    fn unknown_frontmatter_keys_round_trip() {
        let doc = "---\ntype: Playbook\nid: has-extras\ntitle: Has Extras\ncustom_field: hello\nokf_version: \"0.1\"\n---\n\nBody.";
        let entry = markdown_to_entry(doc, None).unwrap();
        assert_eq!(
            entry.extra.get("custom_field").and_then(|v| v.as_str()),
            Some("hello")
        );
        assert_eq!(
            entry.extra.get("okf_version").and_then(|v| v.as_str()),
            Some("0.1")
        );

        let md = entry_to_markdown(&entry).unwrap();
        let recovered = markdown_to_entry(&md, None).unwrap();
        assert_eq!(
            recovered.extra.get("custom_field").and_then(|v| v.as_str()),
            Some("hello")
        );
        assert_eq!(
            recovered.extra.get("okf_version").and_then(|v| v.as_str()),
            Some("0.1")
        );
    }

    #[test]
    fn reserved_filenames_are_recognized() {
        assert!(is_reserved_filename("index.md"));
        assert!(is_reserved_filename("log.md"));
        assert!(!is_reserved_filename("some-concept.md"));
    }
}
