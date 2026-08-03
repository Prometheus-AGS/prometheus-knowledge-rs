use pk_core::types::{ArticleId, WikiEntry};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Lightweight TF-IDF inverted index.
//
// This is intentionally NOT a vector embedding store — that is mempalace-rs'
// domain. This index is fully human-readable and transparent: every score
// is derivable from the markdown files on disk.
//
// The index is rebuilt in-memory at startup from the flat file store, and
// updated incrementally on upsert. No external binary files.
// ---------------------------------------------------------------------------

pub struct TextIndex {
    /// term → { article_id → tf_score }
    inverted: HashMap<String, HashMap<ArticleId, f32>>,
    /// article_id → total term count (for tf normalization)
    doc_lengths: HashMap<ArticleId, usize>,
    /// total document count (for idf)
    doc_count: usize,
}

impl TextIndex {
    pub fn new() -> Self {
        Self {
            inverted: HashMap::new(),
            doc_lengths: HashMap::new(),
            doc_count: 0,
        }
    }

    /// Index or re-index an entry. Safe to call multiple times for the same id.
    pub fn upsert(&mut self, entry: &WikiEntry) {
        self.remove(&entry.id);

        let text = format!("{} {} {}", entry.title, entry.tags.join(" "), entry.content);

        let terms = tokenize(&text);
        let total = terms.len();
        if total == 0 {
            return;
        }

        let mut tf_raw: HashMap<String, usize> = HashMap::new();
        for term in &terms {
            *tf_raw.entry(term.clone()).or_insert(0) += 1;
        }

        for (term, count) in tf_raw {
            let tf = count as f32 / total as f32;
            self.inverted
                .entry(term)
                .or_default()
                .insert(entry.id.clone(), tf);
        }

        self.doc_lengths.insert(entry.id.clone(), total);
        self.doc_count += 1;
    }

    /// Remove an entry from the index.
    pub fn remove(&mut self, id: &ArticleId) {
        if self.doc_lengths.remove(id).is_some() {
            self.doc_count = self.doc_count.saturating_sub(1);
            for postings in self.inverted.values_mut() {
                postings.remove(id);
            }
        }
    }

    /// Return up to `k` article IDs ranked by TF-IDF similarity to `query`.
    pub fn search(&self, query: &str, k: usize) -> Vec<(ArticleId, f32)> {
        if self.doc_count == 0 {
            return Vec::new();
        }

        let query_terms = tokenize(query);
        let mut scores: HashMap<ArticleId, f32> = HashMap::new();

        for term in &query_terms {
            let Some(postings) = self.inverted.get(term) else {
                continue;
            };

            let df = postings.len() as f32;
            let idf = ((self.doc_count as f32) / (df + 1.0)).ln();

            for (id, tf) in postings {
                *scores.entry(id.clone()).or_insert(0.0) += tf * idf;
            }
        }

        let mut ranked: Vec<(ArticleId, f32)> = scores.into_iter().collect();
        ranked.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(k);
        ranked
    }

    pub fn doc_count(&self) -> usize {
        self.doc_count
    }
}

impl Default for TextIndex {
    fn default() -> Self {
        Self::new()
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

static STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "was", "with", "this", "that", "from", "have", "not", "but",
    "they", "their", "been", "more", "also", "its", "can", "will", "one", "all", "any", "has",
    "had", "his", "her", "our", "your", "when",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_search() {
        let mut idx = TextIndex::new();

        let mut e1 = WikiEntry::new(
            "Universal Agent Runtime",
            "Rust async agent execution engine",
        );
        e1.tags = vec!["rust".into(), "agent".into()];

        let mut e2 = WikiEntry::new(
            "mempalace memory system",
            "Episodic memory for agents in Rust",
        );
        e2.tags = vec!["rust".into(), "memory".into()];

        idx.upsert(&e1);
        idx.upsert(&e2);

        let results = idx.search("rust agent runtime", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].0.as_str(), "universal-agent-runtime");
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut idx = TextIndex::new();
        let entry = WikiEntry::new("Test", "hello world test content");
        idx.upsert(&entry);
        idx.upsert(&entry);
        assert_eq!(idx.doc_count(), 1);
    }
}
