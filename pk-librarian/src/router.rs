use crate::client::{LlmClient, LlmConfig};

// ---------------------------------------------------------------------------
// ModelRouter
//
// Routes each LibrarianTask kind to the appropriate model/endpoint.
//   - Compile (high quality needed)   → cloud model via liter-llm gateway
//   - Lint   (cheap, iterative)       → local Qwen via Cherry Studio / mistral.rs
//   - Focus  (medium quality, fast)   → local Qwen
//   - Fix    (cheap, targeted)        → local Qwen
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Compile,
    Lint,
    Focus,
    Fix,
}

#[derive(Debug, Clone)]
pub struct ModelRoute {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub temperature: f32,
}

impl ModelRoute {
    fn local_qwen(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            // Defaults to the local openai-proxy (git@github.com:GQAdonis/openai-proxy.git,
            // systemd --user unit ai.prometheus.openai-proxy.service) bridging Codex CLI auth
            // rather than an LM Studio/Cherry Studio instance, since none runs on this box.
            base_url: std::env::var("LOCAL_LLM_URL")
                .unwrap_or_else(|_| "http://localhost:8181/v1".into()),
            api_key: None,
            temperature: 0.3,
        }
    }

    fn cloud(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            // Defaults to the local openai-proxy rather than api.openai.com so Compile
            // works with zero extra credentials (auth is handled proxy-side via
            // ~/.codex/auth.json).
            base_url: std::env::var("CLOUD_LLM_URL")
                .unwrap_or_else(|_| "http://localhost:8181/v1".into()),
            api_key: std::env::var("CLOUD_LLM_API_KEY").ok(),
            temperature: 0.2,
        }
    }
}

pub struct ModelRouter {
    compile: ModelRoute,
    lint: ModelRoute,
    focus: ModelRoute,
    fix: ModelRoute,
}

impl ModelRouter {
    /// Build from environment variables, with sensible defaults.
    ///
    /// Env vars (all optional):
    ///   PK_COMPILE_MODEL, PK_LINT_MODEL, PK_FOCUS_MODEL, PK_FIX_MODEL
    ///   LOCAL_LLM_URL, CLOUD_LLM_URL, CLOUD_LLM_API_KEY
    pub fn from_env() -> Self {
        Self {
            compile: ModelRoute::cloud(
                std::env::var("PK_COMPILE_MODEL").unwrap_or_else(|_| "gpt-5.5".into()),
            ),
            lint: ModelRoute::local_qwen(
                std::env::var("PK_LINT_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".into()),
            ),
            focus: ModelRoute::local_qwen(
                std::env::var("PK_FOCUS_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".into()),
            ),
            fix: ModelRoute::local_qwen(
                std::env::var("PK_FIX_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".into()),
            ),
        }
    }

    pub fn route(&self, kind: TaskKind) -> &ModelRoute {
        match kind {
            TaskKind::Compile => &self.compile,
            TaskKind::Lint => &self.lint,
            TaskKind::Focus => &self.focus,
            TaskKind::Fix => &self.fix,
        }
    }

    pub fn build_client(&self, kind: TaskKind) -> LlmClient {
        let route = self.route(kind);
        LlmClient::new(LlmConfig {
            base_url: route.base_url.clone(),
            api_key: route.api_key.clone(),
            default_model: route.model.clone(),
            max_tokens: 4096,
            timeout_secs: 180,
        })
    }
}
