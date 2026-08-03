//! SP-002: Sliding-window keyword extraction for long prompts.
//!
//! When a focus topic exceeds LONG_PROMPT_THRESHOLD characters, this module
//! splits it into overlapping windows and aggregates keyword scores across all
//! windows. Keywords from earlier windows decay slightly on each pass so later
//! context can reinforce or override focus.

use std::collections::HashMap;

/// Character threshold above which sliding-window extraction is used.
pub const LONG_PROMPT_THRESHOLD: usize = 2000;

/// Window size in characters.
const WINDOW_SIZE: usize = 1000;

/// Step size (overlap = WINDOW_SIZE - STEP_SIZE).
const STEP_SIZE: usize = 600;

/// Decay factor applied to prior window scores on each new window.
const DECAY: f32 = 0.85;

/// Minimum score for a keyword to be included in the final query.
const MIN_SCORE: f32 = 0.15;

/// Maximum keywords to return.
const MAX_KEYWORDS: usize = 12;

/// Extract keywords from a multi-turn conversation.
///
/// The input is a newline-separated sequence of turns. The last `n_turns` are
/// used. Keywords from earlier turns decay per DECAY; keywords from later turns
/// can reinforce or override earlier focus. Returns a compact query string.
pub fn extract_query_multi_turn(turns_text: &str, n_turns: usize) -> String {
    let turns: Vec<&str> = turns_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let start = if turns.len() > n_turns {
        turns.len() - n_turns
    } else {
        0
    };
    let active_turns = &turns[start..];

    if active_turns.is_empty() {
        return turns_text.to_owned();
    }

    let total = active_turns.len();
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (turn_idx, turn) in active_turns.iter().enumerate() {
        let window_keywords = extract_from_window(turn);
        // Earlier turns decay more (turn 0 is oldest, turn total-1 is newest)
        let age = (total - 1 - turn_idx) as i32;
        let decay_factor = DECAY.powi(age);

        for (kw, score) in window_keywords {
            let entry = scores.entry(kw).or_insert(0.0);
            *entry += score * decay_factor;
        }
    }

    let mut ranked: Vec<(String, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let cutoff = ranked.first().map(|(_, s)| s * 0.1_f32).unwrap_or(0.0);
    ranked.retain(|(_, s)| *s >= cutoff);
    ranked.truncate(MAX_KEYWORDS);

    if ranked.is_empty() {
        return active_turns
            .last()
            .copied()
            .unwrap_or(turns_text)
            .to_owned();
    }

    ranked
        .into_iter()
        .map(|(kw, _)| kw)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract keywords from a potentially long prompt using a sliding window.
///
/// Returns a compact query string suitable for vector search. If the prompt is
/// short, it is returned as-is (single-window fast path).
pub fn extract_query(prompt: &str) -> String {
    if prompt.chars().count() <= LONG_PROMPT_THRESHOLD {
        return prompt.to_owned();
    }

    let chars: Vec<char> = prompt.chars().collect();
    let total = chars.len();
    let mut scores: HashMap<String, f32> = HashMap::new();
    let mut window_idx = 0usize;

    let mut start = 0usize;
    while start < total {
        let end = (start + WINDOW_SIZE).min(total);
        let window: String = chars[start..end].iter().collect();

        let keywords = extract_from_window(&window);
        let decay = DECAY.powi(window_idx as i32);

        for (kw, score) in keywords {
            let entry = scores.entry(kw).or_insert(0.0);
            // Later windows reinforce (no decay) or introduce new keywords
            if window_idx == 0 {
                *entry += score;
            } else {
                // Score existing: decay old, add new contribution
                *entry = (*entry * decay) + score;
            }
        }

        if end >= total {
            break;
        }
        start += STEP_SIZE;
        window_idx += 1;
    }

    // Sort by score descending, take top MAX_KEYWORDS
    let mut ranked: Vec<(String, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Apply MIN_SCORE only after ensuring we have at least 1 result
    let cutoff = if let Some((_, top_score)) = ranked.first() {
        // Dynamic threshold: at least MIN_SCORE, but not above 10% of top score
        top_score * 0.1_f32
    } else {
        return prompt[..LONG_PROMPT_THRESHOLD.min(prompt.len())].to_owned();
    };

    ranked.retain(|(_, s)| *s >= cutoff.max(MIN_SCORE * 0.1));
    ranked.truncate(MAX_KEYWORDS);

    if ranked.is_empty() {
        return prompt[..LONG_PROMPT_THRESHOLD.min(prompt.len())].to_owned();
    }

    ranked
        .into_iter()
        .map(|(kw, _)| kw)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Simple heuristic keyword extraction from a single window.
///
/// Scores tokens by:
/// - Length (longer tokens preferred over stop words)
/// - CamelCase or underscore_case (identifier heuristic)
/// - Repetition within window
/// - NOT in common stop word list
fn extract_from_window(window: &str) -> Vec<(String, f32)> {
    let stop_words = stop_word_set();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for raw_token in window.split_whitespace() {
        // Strip punctuation
        let token: String = raw_token
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if token.len() < 3 {
            continue;
        }
        let lower = token.to_lowercase();
        if stop_words.contains(lower.as_str()) {
            continue;
        }
        *counts.entry(token).or_insert(0) += 1;
    }

    let total_tokens = counts.values().sum::<usize>().max(1) as f32;

    counts
        .into_iter()
        .map(|(kw, count)| {
            let tf = count as f32 / total_tokens;
            let length_bonus = if kw.len() >= 6 { 1.5 } else { 1.0 };
            let identifier_bonus = if kw.contains('_') || kw.contains('-') || is_camel_case(&kw) {
                1.3
            } else {
                1.0
            };
            (kw, tf * length_bonus * identifier_bonus)
        })
        .collect()
}

fn is_camel_case(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.len() > 3
        && chars[0].is_ascii_lowercase()
        && chars[1..].iter().any(|c| c.is_ascii_uppercase())
}

fn stop_word_set() -> &'static std::collections::HashSet<&'static str> {
    use std::sync::OnceLock;
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was",
            "one", "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now",
            "old", "see", "two", "way", "who", "boy", "did", "its", "let", "put", "say", "she",
            "too", "use", "that", "this", "with", "have", "from", "they", "will", "been", "when",
            "what", "your", "each", "which", "their", "time", "more", "very", "then", "than",
            "some", "would", "make", "into", "like", "over", "such", "also", "back", "after",
            "just", "where", "most", "know", "take", "year", "good", "much", "before", "right",
            "look", "think", "even", "through", "work", "long", "here", "well", "down", "were",
            "same", "about", "these", "should", "being", "used", "using", "returns", "return",
            "param", "params", "type", "types", "true", "false", "null", "none", "error", "result",
            "value", "values",
        ]
        .iter()
        .cloned()
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_prompt_returned_as_is() {
        let prompt = "kubernetes deployment";
        assert_eq!(extract_query(prompt), prompt);
    }

    #[test]
    fn long_prompt_extracts_keywords() {
        let long = "user_authentication ".repeat(300);
        let result = extract_query(&long);
        assert!(
            result.contains("user_authentication"),
            "expected keyword in: {result}"
        );
        assert!(
            result.len() < long.len(),
            "expected shorter output than input"
        );
    }

    #[test]
    fn deduplicates_across_windows() {
        // Repeated technical term should score high
        let repeated = "SurrealDB ".repeat(200) + &" PostgreSQL ".repeat(100);
        let result = extract_query(&(repeated + &"x".repeat(500)));
        assert!(
            result.contains("SurrealDB")
                || result.contains("surrealdb")
                || result.to_lowercase().contains("surrealdb")
        );
    }

    #[test]
    fn multi_turn_latest_turn_dominates() {
        // First turn about Rust, last turn about SurrealDB — SurrealDB should win
        let turns =
            "rust ownership borrowing lifetimes\nSurrealDB SurrealDB SurrealDB schema tables";
        let result = extract_query_multi_turn(turns, 10);
        assert!(
            result.to_lowercase().contains("surrealdb"),
            "latest turn should dominate: got '{result}'"
        );
    }

    #[test]
    fn multi_turn_n_turns_limits_context() {
        // 5 turns total but we only use last 2
        let turns = "rust\nrust\nrust\nSurrealDB tables schema\nSurrealDB schema migrations";
        let result = extract_query_multi_turn(turns, 2);
        assert!(
            result.to_lowercase().contains("surrealdb"),
            "should use last 2 turns: got '{result}'"
        );
    }

    #[test]
    fn max_keywords_respected() {
        // 500 unique tokens * ~14 chars each = ~7000 chars, well above threshold
        let varied: String = (0..500).map(|i| format!("uniqueKeyword{i:03} ")).collect();
        assert!(
            varied.len() > LONG_PROMPT_THRESHOLD,
            "test setup: prompt must be long"
        );
        let result = extract_query(&varied);
        // Result is space-joined keywords; each keyword is a single token
        let count = result.split_whitespace().count();
        assert!(
            count <= MAX_KEYWORDS,
            "expected ≤ {MAX_KEYWORDS} keywords, got {count}: '{result}'"
        );
    }
}
