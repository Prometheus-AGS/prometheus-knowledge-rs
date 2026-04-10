use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, WikiEntry},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    id: String,
    title: String,
    tags: Vec<String>,
    links: Vec<String>,
    sources: Vec<String>,
    created_at: String,
    updated_at: String,
    revision: u32,
}

pub fn entry_to_markdown(entry: &WikiEntry) -> PkResult<String> {
    let fm = Frontmatter {
        id: entry.id.as_str().to_owned(),
        title: entry.title.clone(),
        tags: entry.tags.clone(),
        links: entry.links.iter().map(|l| l.as_str().to_owned()).collect(),
        sources: entry.sources.clone(),
        created_at: entry.created_at.to_rfc3339(),
        updated_at: entry.updated_at.to_rfc3339(),
        revision: entry.revision,
    };

    let yaml = serde_yaml::to_string(&fm)
        .map_err(|e| PkError::frontmatter(e.to_string()))?;

    Ok(format!("---\n{}---\n\n{}", yaml, entry.content))
}

pub fn markdown_to_entry(raw: &str) -> PkResult<WikiEntry> {
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

    let created_at = chrono::DateTime::parse_from_rfc3339(&fm.created_at)
        .map_err(|e| PkError::frontmatter(format!("created_at: {e}")))?
        .with_timezone(&chrono::Utc);

    let updated_at = chrono::DateTime::parse_from_rfc3339(&fm.updated_at)
        .map_err(|e| PkError::frontmatter(format!("updated_at: {e}")))?
        .with_timezone(&chrono::Utc);

    Ok(WikiEntry {
        id: ArticleId::from(fm.id),
        title: fm.title,
        content,
        tags: fm.tags,
        links: fm.links.into_iter().map(ArticleId::from).collect(),
        sources: fm.sources,
        created_at,
        updated_at,
        revision: fm.revision,
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

        let recovered = markdown_to_entry(&md).unwrap();
        assert_eq!(recovered.id, entry.id);
        assert_eq!(recovered.title, entry.title);
        assert_eq!(recovered.content, entry.content);
        assert_eq!(recovered.tags, entry.tags);
        assert_eq!(recovered.revision, entry.revision);
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let result = markdown_to_entry("# No frontmatter here\n\nJust a body.");
        assert!(result.is_err());
    }
}
