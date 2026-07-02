use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ArticleId — zero-cost newtype, repr(transparent) for safe transmutation
// and zero-overhead FFI at store boundaries.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ArticleId(pub String);

impl ArticleId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_slug(title: &str) -> Self {
        let slug = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>();
        // Collapse consecutive dashes
        let slug = slug
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        Self(slug)
    }

    #[inline(always)]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ArticleId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ArticleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ArticleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ArticleId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// WikiEntry — a compiled, structured knowledge article maintained by the
// Librarian. Stored as a Markdown file with YAML frontmatter.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiEntry {
    pub id: ArticleId,
    pub title: String,

    /// Rendered markdown body (minus frontmatter)
    pub content: String,

    /// Topic tags used for search and linking
    pub tags: Vec<String>,

    /// Outbound links to other ArticleIds — the Librarian maintains this
    pub links: Vec<ArticleId>,

    /// Where this knowledge came from (file path, URL, agent session ID, etc.)
    pub sources: Vec<String>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    /// Revision counter — incremented on every upsert
    pub revision: u32,

    /// Open Knowledge Format (OKF) v0.1 §4.1 `type` — the format's one
    /// required frontmatter key. `None` for entries compiled before OKF
    /// adoption or lacking a producer-assigned type.
    #[serde(default)]
    pub entry_type: Option<String>,

    /// Frontmatter keys pk does not model structurally (OKF producer
    /// extensions, or fields from a future OKF minor version). Preserved
    /// verbatim across parse → serialize round-trips per OKF §9's permissive
    /// consumption rule — unknown keys are never grounds to drop data.
    #[serde(default)]
    pub extra: std::collections::BTreeMap<String, serde_yaml::Value>,
}

impl WikiEntry {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        let now = Utc::now();
        let title = title.into();
        Self {
            id: ArticleId::from_slug(&title),
            title,
            content: content.into(),
            tags: Vec::new(),
            links: Vec::new(),
            sources: Vec::new(),
            created_at: now,
            updated_at: now,
            revision: 0,
            entry_type: None,
            extra: std::collections::BTreeMap::new(),
        }
    }

    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(|t| t.into()).collect();
        self
    }

    pub fn with_sources(mut self, sources: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.sources = sources.into_iter().map(|s| s.into()).collect();
        self
    }

    pub fn bump_revision(&mut self) {
        self.revision += 1;
        self.updated_at = Utc::now();
    }
}

// ---------------------------------------------------------------------------
// RawDoc — an unprocessed document dropped into the `raw/` inbox directory.
// The file watcher emits these; the Librarian compiles them into WikiEntries.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDoc {
    pub id: String,
    pub source_path: String,
    pub content: String,
    pub media_type: RawDocMediaType,
    pub ingested_at: DateTime<Utc>,
    /// Optional hint: the agent session / conversation that produced this doc
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawDocMediaType {
    Markdown,
    PlainText,
    Json,
    Url,
}

impl RawDoc {
    pub fn from_path(path: impl Into<String>, content: impl Into<String>) -> Self {
        let source_path = path.into();
        let media_type = if source_path.ends_with(".md") {
            RawDocMediaType::Markdown
        } else if source_path.ends_with(".json") {
            RawDocMediaType::Json
        } else {
            RawDocMediaType::PlainText
        };
        Self {
            id: Uuid::new_v4().to_string(),
            source_path,
            content: content.into(),
            media_type,
            ingested_at: Utc::now(),
            session_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// LintReport — a single issue found by the Librarian's lint pass.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintReport {
    /// Which article this issue is in (None = global / cross-article)
    pub entry_id: Option<ArticleId>,
    pub severity: LintSeverity,
    pub issue: String,
    pub suggestion: String,
    pub auto_fixable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LintSeverity::Info => write!(f, "info"),
            LintSeverity::Warning => write!(f, "warning"),
            LintSeverity::Error => write!(f, "error"),
        }
    }
}
