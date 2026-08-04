//! SP-010: Strict JSON response parser for LLM outputs.
//!
//! Replaces the previous inline serde_json calls with a typed ParseError
//! that surfaces every failure path explicitly. No silent fallback to empty.

use std::fmt;

/// Structured parse error for LLM response parsing.
#[derive(Debug)]
pub enum ParseError {
    /// The JSON fence stripping produced an empty string.
    EmptyResponse,
    /// serde_json deserialization failed.
    InvalidJson { message: String, raw: String },
    /// Required field is missing or null after deserialization.
    MissingField { field: &'static str, raw: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::EmptyResponse => {
                write!(f, "LLM returned an empty response after stripping fences")
            }
            ParseError::InvalidJson { message, raw } => {
                write!(
                    f,
                    "JSON parse failed: {message}\nRaw (first 500 chars): {}",
                    &raw[..raw.len().min(500)]
                )
            }
            ParseError::MissingField { field, raw } => {
                write!(
                    f,
                    "Required field '{field}' missing from LLM response\nRaw (first 500 chars): {}",
                    &raw[..raw.len().min(500)]
                )
            }
        }
    }
}

/// Strip markdown code fences from an LLM response.
///
/// Returns `Err(ParseError::EmptyResponse)` if the result is empty after
/// stripping — never returns an empty string silently.
pub fn strip_fences(s: &str) -> Result<&str, ParseError> {
    let s = s.trim();
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    let s = s.trim();
    if s.is_empty() {
        Err(ParseError::EmptyResponse)
    } else {
        Ok(s)
    }
}

/// Parse a JSON string into `T`, returning a typed `ParseError` on failure.
pub fn parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, ParseError> {
    let json_str = strip_fences(raw)?;
    serde_json::from_str(json_str).map_err(|e| ParseError::InvalidJson {
        message: e.to_string(),
        raw: json_str.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Sample {
        title: String,
        #[serde(default)]
        tags: Vec<String>,
    }

    #[test]
    fn parses_plain_json() {
        let raw = r#"{"title": "hello", "tags": ["a", "b"]}"#;
        let s: Sample = parse_json(raw).unwrap();
        assert_eq!(s.title, "hello");
        assert_eq!(s.tags, vec!["a", "b"]);
    }

    #[test]
    fn parses_fenced_json() {
        let raw = "```json\n{\"title\": \"world\"}\n```";
        let s: Sample = parse_json(raw).unwrap();
        assert_eq!(s.title, "world");
    }

    #[test]
    fn empty_response_returns_error() {
        let result: Result<Sample, _> = parse_json("```\n```");
        assert!(matches!(result, Err(ParseError::EmptyResponse)));
    }

    #[test]
    fn invalid_json_surfaces_message() {
        let result: Result<Sample, _> = parse_json(r#"{"title": bad_json"#);
        match result {
            Err(ParseError::InvalidJson { message, .. }) => {
                assert!(!message.is_empty());
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_field_surfaces_error() {
        // title is not optional — serde will return an error
        let result: Result<Sample, _> = parse_json(r#"{"tags": ["a"]}"#);
        assert!(
            matches!(result, Err(ParseError::InvalidJson { .. })),
            "missing required field should fail"
        );
    }
}
