//! Content-identity helpers for ingest-time duplicate detection.
//!
//! `Librarian::compile()` synthesizes a fresh title (and therefore a fresh
//! `ArticleId`, since `WikiEntry::new` derives the id from the title slug)
//! on every call, even when the underlying raw content repeats. Without an
//! identity anchored to *content*, repeated ingestion of the same fact
//! produces a new wiki entry every time. These helpers give the store a
//! content-keyed identity so the compile path can merge into an existing
//! entry instead.

use pk_core::types::{ArticleId, WikiEntry};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Frontmatter key under which the normalized content hash is stored in
/// `WikiEntry.extra` — an OKF producer-extension field (OKF §9's permissive
/// consumption rule preserves unknown keys verbatim), not part of the
/// OKF-required schema.
pub const CONTENT_HASH_KEY: &str = "content_hash";

/// Similarity ratio (Jaccard over normalized word sets) above which two
/// documents are treated as near-duplicates. Deliberately conservative to
/// avoid merging genuinely distinct content.
pub const NEAR_DUPLICATE_THRESHOLD: f32 = 0.85;

/// Stamp the normalized content hash onto an entry's `extra` frontmatter map
/// (see `CONTENT_HASH_KEY`), so it round-trips through `upsert()` and the
/// on-disk markdown alongside the rest of the entry.
pub fn stamp_content_hash(entry: &mut WikiEntry, hash: &str) {
    entry.extra.insert(
        CONTENT_HASH_KEY.to_string(),
        serde_yaml::Value::String(hash.to_string()),
    );
}

/// Hash raw content after normalizing incidental whitespace, so two ingests
/// of "the same" content hash identically even when formatting drifts
/// slightly between calls (trailing newline, double space, etc).
pub fn normalized_content_hash(content: &str) -> String {
    let normalized = normalize_words(content).join(" ");
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

/// Jaccard similarity between the normalized word sets of two documents —
/// a bounded `[0.0, 1.0]` measure independent of corpus size, unlike the
/// unnormalized TF-IDF sum `TextIndex::search` returns.
pub fn word_overlap_ratio(a: &str, b: &str) -> f32 {
    let words_a: HashSet<String> = normalize_words(a).into_iter().collect();
    let words_b: HashSet<String> = normalize_words(b).into_iter().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Best-scoring candidate whose content is a near-duplicate of `content`
/// (ratio at or above `NEAR_DUPLICATE_THRESHOLD`), if any.
pub fn find_near_duplicate(content: &str, candidates: &[WikiEntry]) -> Option<ArticleId> {
    candidates
        .iter()
        .map(|entry| (entry, word_overlap_ratio(content, &entry.content)))
        .filter(|(_, ratio)| *ratio >= NEAR_DUPLICATE_THRESHOLD)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(entry, _)| entry.id.clone())
}

fn normalize_words(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_hashes_equal() {
        let a = "executor session complete | phase: foo | change: unknown";
        let b = "executor session complete | phase: foo | change: unknown";
        assert_eq!(normalized_content_hash(a), normalized_content_hash(b));
    }

    #[test]
    fn whitespace_only_differences_hash_equal() {
        let a = "executor session complete | phase: foo | change: unknown";
        let b = "executor session complete | phase: foo | change: unknown\n\n  ";
        assert_eq!(normalized_content_hash(a), normalized_content_hash(b));
    }

    #[test]
    fn distinct_content_hashes_differ() {
        let a = "executor session complete | phase: foo | change: unknown";
        let b = "executor session complete | phase: bar | change: unknown";
        assert_ne!(normalized_content_hash(a), normalized_content_hash(b));
    }

    #[test]
    fn near_identical_wording_scores_above_threshold() {
        let a = "An executor session completed for the adversarial review for creation phase.";
        let b = "An executor session completed for the adversarial review creation phase.";
        assert!(word_overlap_ratio(a, b) >= NEAR_DUPLICATE_THRESHOLD);
    }

    #[test]
    fn unrelated_content_scores_below_threshold() {
        let a = "An executor session completed for the adversarial review for creation phase.";
        let b = "The axum router now validates JWT bearer tokens on every request.";
        assert!(word_overlap_ratio(a, b) < NEAR_DUPLICATE_THRESHOLD);
    }

    #[test]
    fn find_near_duplicate_returns_best_match_above_threshold() {
        let near = WikiEntry::new(
            "Adversarial Review for Creation Completion Record",
            "An executor session completed for the adversarial review for creation phase.",
        );
        let unrelated = WikiEntry::new(
            "Axum Router JWT",
            "The axum router validates bearer tokens.",
        );
        let candidates = vec![unrelated, near.clone()];

        let found = find_near_duplicate(
            "An executor session completed for the adversarial review creation phase.",
            &candidates,
        );
        assert_eq!(found, Some(near.id));
    }

    #[test]
    fn stamp_content_hash_round_trips_through_extra() {
        let mut entry = WikiEntry::new("Title", "body");
        stamp_content_hash(&mut entry, "abc123");
        assert_eq!(
            entry.extra.get(CONTENT_HASH_KEY).and_then(|v| v.as_str()),
            Some("abc123")
        );
    }

    #[test]
    fn find_near_duplicate_returns_none_when_nothing_matches() {
        let unrelated = WikiEntry::new(
            "Axum Router JWT",
            "The axum router validates bearer tokens.",
        );
        let found = find_near_duplicate(
            "Completely unrelated new fact about databases.",
            &[unrelated],
        );
        assert_eq!(found, None);
    }
}
