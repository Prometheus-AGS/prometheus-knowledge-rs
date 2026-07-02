use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// LlmClient
//
// Thin async wrapper around any OpenAI-compatible `/v1/chat/completions`
// endpoint. Works standalone (Cherry Studio, LM Studio, mistral.rs) or
// pointed at the UAR's liter-llm gateway for full provider routing.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: String,
    pub max_tokens: u32,
    pub timeout_secs: u64,
}

impl LlmConfig {
    pub fn cherry_local(model: impl Into<String>) -> Self {
        Self {
            base_url: "http://localhost:1234/v1".into(),
            api_key: None,
            default_model: model.into(),
            max_tokens: 4096,
            timeout_secs: 120,
        }
    }

    pub fn openai_compat(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: Some(api_key.into()),
            default_model: model.into(),
            max_tokens: 4096,
            timeout_secs: 120,
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    // Some providers return `"content": null` (e.g. reasoning models that
    // exhaust max_tokens before emitting text, or tool-call-only turns).
    content: Option<String>,
}

/// First ~300 chars of a response body, flattened, for error messages.
fn body_snippet(body: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "(empty body)".into();
    }
    if flat.len() <= 300 {
        return flat;
    }
    let mut end = 300;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &flat[..end])
}

/// Saves a full response body that failed to parse to a scratch file for
/// forensic inspection, since `body_snippet` only keeps the first 300 chars.
/// Best-effort: returns `None` on any I/O failure rather than masking the
/// original parse error.
fn dump_body_for_forensics(body: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("pk-llm-parse-failures");
    std::fs::create_dir_all(&dir).ok()?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f");
    let path = dir.join(format!("{ts}.txt"));
    std::fs::write(&path, body).ok()?;
    tracing::error!(path = %path.display(), len = body.len(), "LLM response failed to parse; full body saved");
    Some(path)
}

#[derive(Clone)]
pub struct LlmClient {
    config: LlmConfig,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let mut builder = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(config.timeout_secs));

        if let Some(key) = &config.api_key {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {key}").parse().unwrap(),
            );
            builder = builder.default_headers(headers);
        }

        Self {
            config,
            http: builder.build().expect("reqwest client build"),
        }
    }

    pub async fn complete(
        &self,
        system: &str,
        user: &str,
        model_override: Option<&str>,
        temperature: f32,
    ) -> Result<String> {
        let model = model_override.unwrap_or(&self.config.default_model);
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));

        let req = ChatRequest {
            model,
            messages: vec![
                ChatMessage { role: "system", content: system },
                ChatMessage { role: "user", content: user },
            ],
            max_tokens: self.config.max_tokens,
            temperature,
        };

        let http_resp = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .context("LLM request failed")?;

        let status = http_resp.status();
        let content_length_header = http_resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        let body = http_resp
            .text()
            .await
            .context("failed to read LLM response body")?;

        if !status.is_success() {
            anyhow::bail!(
                "LLM returned error status {status}: {}",
                body_snippet(&body)
            );
        }

        let resp: ChatResponse = serde_json::from_str(&body).with_context(|| {
            let dump_path = dump_body_for_forensics(&body);
            let len_mismatch = content_length_header
                .filter(|&expected| expected != body.len())
                .map(|expected| format!(", content-length header {expected} != body bytes read {}", body.len()))
                .unwrap_or_default();
            format!(
                "failed to parse LLM response (status {status}, body: {}{len_mismatch}{})",
                body_snippet(&body),
                dump_path
                    .map(|p| format!(", full body saved to {}", p.display()))
                    .unwrap_or_default()
            )
        })?;

        resp.choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "LLM returned no choices or empty content (status {status}, body: {})",
                    body_snippet(&body)
                )
            })
    }
}
