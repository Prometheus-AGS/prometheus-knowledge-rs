use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, WikiEntry},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Filenames reserved by the Open Knowledge Format (OKF v0.1 §3.1) at any
/// level of a bundle. Never treated as concept documents.
pub const RESERVED_FILENAMES: [&str; 2] = ["index.md", "log.md"];

pub fn is_reserved_filename(name: &str) -> bool {
    RESERVED_FILENAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// Frontmatter — permissive per OKF v0.1 §4.1 and §9. `type` is OKF's one
// required key; every pk-native field is optional so both a minimal OKF
// document and a legacy pre-OKF pk document parse without error. Keys this
// struct doesn't model are captured in `extra` and preserved verbatim on
// round-trip (OKF §9: unknown keys are never grounds to drop data).
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
    sources: Vec<String>,
    /// OKF §4.1 `timestamp` — mirrors pk's `updated_at` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        // OKF §4.1 `timestamp` mirrors pk's `updated_at`, which is also
        // written below as a pk extension key for full round-trip fidelity.
        timestamp: Some(entry.updated_at.to_rfc3339()),
        created_at: Some(entry.created_at.to_rfc3339()),
        updated_at: Some(entry.updated_at.to_rfc3339()),
        revision: Some(entry.revision),
        extra: entry.extra.clone(),
    };

    let yaml = serde_yaml::to_string(&fm)
        .map_err(|e| PkError::frontmatter(e.to_string()))?;

    Ok(format!("---\n{}---\n\n{}", yaml, entry.content))
}

/// Parse a markdown document into a `WikiEntry`.
///
/// `fallback_id` supplies the concept ID when frontmatter omits `id` (as any
/// conformant OKF document may) — callers with a file path pass the
/// wiki-relative path (minus `.md`) per OKF §2's Concept ID definition.
pub fn markdown_to_entry(raw: &str, fallback_id: Option<&str>) -> PkResult<WikiEntry> {
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
            "id {id:?} is not a safe concept path (no leading '/', no '..' or empty segments)"
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
    // OKF's `timestamp` is the closest equivalent to pk's `updated_at`;
    // prefer an explicit `updated_at` (pk-native) over `timestamp` (OKF).
    let updated_at = match fm.updated_at.as_ref().or(fm.timestamp.as_ref()) {
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|e| PkError::frontmatter(format!("updated_at/timestamp: {e}")))?
            .with_timezone(&chrono::Utc),
        None => now,
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
        let entry = WikiEntry::new("Universal Agent Runtime", "The UAR is the core of Prometheus.")
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

    #[test]
    fn rejects_missing_frontmatter() {
        let result = markdown_to_entry("# No frontmatter here\n\nJust a body.", None);
        assert!(result.is_err());
    }

    /// OKF v0.1 §9: a bundle is conformant if every frontmatter block has a
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

    /// OKF §9: unknown frontmatter keys must survive a round-trip, not be
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
