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
    content: String,
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

        let resp: ChatResponse = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .await
            .context("LLM request failed")?
            .error_for_status()
            .context("LLM returned error status")?
            .json()
            .await
            .context("failed to parse LLM response")?;

        resp.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow::anyhow!("LLM returned no choices"))
    }
}
