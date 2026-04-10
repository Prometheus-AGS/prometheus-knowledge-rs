use crate::registry::{AgentTool, KnowledgeTool, ToolCall, ToolResult};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::warn;

// ---------------------------------------------------------------------------
// UarMcpClient — HTTP JSON-RPC client for a running pk-cherry instance
//
// Use when pk-cherry runs as a separate process or K3s pod.
// Multiple UAR instances can share one pk-cherry over HTTP.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct UarMcpClient {
    base_url: String,
    http: reqwest::Client,
    next_id: Arc<AtomicU64>,
}

impl UarMcpClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("reqwest client"),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id();
        let body = json!({ "method": method, "params": params, "id": id });

        let resp: serde_json::Value = self
            .http
            .post(format!("{}/mcp", self.base_url))
            .json(&body)
            .send().await?
            .error_for_status()?
            .json().await?;

        if let Some(err) = resp.get("error") {
            return Err(anyhow::anyhow!("MCP error: {err}"));
        }

        Ok(resp.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    pub fn tools(self: Arc<Self>) -> Vec<Box<dyn AgentTool>> {
        KnowledgeTool::all()
            .iter()
            .map(|&kind| {
                Box::new(RemoteTool { kind, client: Arc::clone(&self) }) as Box<dyn AgentTool>
            })
            .collect()
    }

    pub async fn health_check(&self) -> bool {
        match self.http.get(format!("{}/health", self.base_url)).send().await {
            Ok(r) => r.status().is_success(),
            Err(e) => { warn!(err = %e, "pk-cherry health check failed"); false }
        }
    }
}

struct RemoteTool {
    kind: KnowledgeTool,
    client: Arc<UarMcpClient>,
}

#[async_trait::async_trait]
impl AgentTool for RemoteTool {
    fn name(&self) -> &str { self.kind.name() }
    fn description(&self) -> &str { self.kind.description() }
    fn input_schema(&self) -> serde_json::Value { self.kind.input_schema() }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        match self.client.call(self.kind.name(), call.input).await {
            Ok(result) => {
                let content = result
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default());
                ToolResult::ok(call.id, content)
            }
            Err(e) => ToolResult::err(call.id, e.to_string()),
        }
    }
}

pub fn anthropic_tool_definitions() -> serde_json::Value {
    let tools: Vec<serde_json::Value> = KnowledgeTool::all()
        .iter().map(|t| t.anthropic_tool_def()).collect();
    json!({ "tools": tools })
}
