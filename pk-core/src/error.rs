use thiserror::Error;

#[derive(Debug, Error)]
pub enum PkError {
    #[error("article not found: {0}")]
    NotFound(String),

    #[error("store I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("frontmatter parse error: {0}")]
    Frontmatter(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("watcher error: {0}")]
    Watcher(String),

    #[error("{0}")]
    Other(String),
}

// Keep error construction cold — these are never on the hot path.
impl PkError {
    #[cold]
    #[inline(never)]
    pub fn not_found(id: impl std::fmt::Display) -> Self {
        Self::NotFound(id.to_string())
    }

    #[cold]
    #[inline(never)]
    pub fn frontmatter(msg: impl std::fmt::Display) -> Self {
        Self::Frontmatter(msg.to_string())
    }

    #[cold]
    #[inline(never)]
    pub fn llm(msg: impl std::fmt::Display) -> Self {
        Self::Llm(msg.to_string())
    }
}

pub type PkResult<T> = Result<T, PkError>;
