use serde::{Deserialize, Serialize};

/// A single tool call from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Result returned to the LLM after execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self { tool_use_id: id.into(), content: content.into(), is_error: false }
    }

    #[cold]
    #[inline(never)]
    pub fn err(id: impl Into<String>, msg: impl std::fmt::Display) -> Self {
        Self { tool_use_id: id.into(), content: msg.to_string(), is_error: true }
    }
}

/// Trait that both UarToolProvider (in-process) and UarMcpClient (remote) implement.
#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    async fn execute(&self, call: ToolCall) -> ToolResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeTool {
    Ingest,
    Lint,
    Focus,
    Search,
    Get,
}

impl KnowledgeTool {
    pub fn all() -> &'static [KnowledgeTool] {
        &[
            KnowledgeTool::Ingest,
            KnowledgeTool::Lint,
            KnowledgeTool::Focus,
            KnowledgeTool::Search,
            KnowledgeTool::Get,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Ingest => "knowledge_ingest",
            Self::Lint   => "knowledge_lint",
            Self::Focus  => "knowledge_focus",
            Self::Search => "knowledge_search",
            Self::Get    => "knowledge_get",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Ingest => "Compile raw content (notes, code, session transcripts) into the persistent engineering knowledge base.",
            Self::Lint   => "Scan the knowledge base for gaps, contradictions, orphaned articles, and missing cross-links.",
            Self::Focus  => "Synthesize a focused mini-knowledge-base for a topic. Returns dense markdown optimised for context loading. Use this at the start of any session to load relevant prior knowledge.",
            Self::Search => "Full-text search the knowledge base. Returns ranked article summaries.",
            Self::Get    => "Retrieve a full wiki article by its ID slug.",
        }
    }

    pub fn input_schema(&self) -> serde_json::Value {
        match self {
            Self::Ingest => serde_json::json!({
                "type": "object", "required": ["content"],
                "properties": {
                    "content": { "type": "string", "description": "Raw markdown, notes, code comments, or plain text to compile" },
                    "source":  { "type": "string", "description": "Source label (file path, session ID, URL)" }
                }
            }),
            Self::Lint => serde_json::json!({ "type": "object", "properties": {} }),
            Self::Focus => serde_json::json!({
                "type": "object", "required": ["topic"],
                "properties": {
                    "topic": { "type": "string", "description": "Topic or question to build context around" },
                    "k": { "type": "integer", "description": "Max source articles to synthesize (default 10)", "default": 10 }
                }
            }),
            Self::Search => serde_json::json!({
                "type": "object", "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "Search terms" },
                    "k": { "type": "integer", "description": "Max results (default 5)", "default": 5 }
                }
            }),
            Self::Get => serde_json::json!({
                "type": "object", "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "Article ID slug (e.g. 'universal-agent-runtime')" }
                }
            }),
        }
    }

    pub fn anthropic_tool_def(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name(),
            "description": self.description(),
            "input_schema": self.input_schema()
        })
    }
}
