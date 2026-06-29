use pk_core::types::{ArticleId, RawDoc, WikiEntry};
use pk_librarian::Librarian;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct McpRequest {
    pub method: String,
    // Tolerant: `initialize`/notifications may omit params, notifications omit id.
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct McpResponse {
    // JSON-RPC 2.0 requires this on every response; strict MCP clients reject its absence.
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

impl McpResponse {
    pub fn ok(id: serde_json::Value, result: impl Serialize) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(serde_json::to_value(result).unwrap_or_default()),
            error: None,
            id,
        }
    }

    #[cold]
    #[inline(never)]
    pub fn err(id: serde_json::Value, code: i32, msg: impl std::fmt::Display) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(McpError { code, message: msg.to_string() }),
            id,
        }
    }
}

pub fn tool_list() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "knowledge_ingest",
                "description": "Ingest raw content into the knowledge base. The Librarian will compile it into a structured wiki entry.",
                "inputSchema": {
                    "type": "object",
                    "required": ["content"],
                    "properties": {
                        "content": { "type": "string", "description": "Raw markdown, notes, or plain text to ingest" },
                        "source": { "type": "string", "description": "Optional source identifier (file path, URL, session ID)" },
                        "session_id": { "type": "string", "description": "Optional agent session ID for provenance tracking" }
                    }
                }
            },
            {
                "name": "knowledge_lint",
                "description": "Scan the full knowledge base for gaps, contradictions, orphaned articles, and missing links.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "knowledge_focus",
                "description": "Build a focused mini-knowledge-base for a specific topic. Returns dense markdown optimized for LLM context loading.",
                "inputSchema": {
                    "type": "object",
                    "required": ["topic"],
                    "properties": {
                        "topic": { "type": "string", "description": "Topic or question to focus on" },
                        "k": { "type": "integer", "description": "Max number of source articles to synthesize (default 10)" }
                    }
                }
            },
            {
                "name": "knowledge_search",
                "description": "Full-text search the knowledge base. Returns ranked wiki entries.",
                "inputSchema": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "k": { "type": "integer", "description": "Max results (default 5)" }
                    }
                }
            },
            {
                "name": "knowledge_get",
                "description": "Retrieve a single wiki entry by its article ID (slug).",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "description": "Article slug (e.g. 'universal-agent-runtime')" }
                    }
                }
            }
        ]
    })
}

pub async fn dispatch(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    match req.method.as_str() {
        // MCP lifecycle — required so spec-compliant clients (Claude Code, etc.)
        // complete the Streamable HTTP handshake before listing/calling tools.
        "initialize" => McpResponse::ok(
            req.id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "prometheus-knowledge", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        "ping" => McpResponse::ok(req.id, serde_json::json!({})),
        "tools/list" => McpResponse::ok(req.id, tool_list()),

        // Standard MCP tool invocation: params = { name, arguments }.
        "tools/call" => {
            let name = req.params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let arguments = req.params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
            let sub = McpRequest { method: name.clone(), params: arguments, id: req.id.clone() };
            match name.as_str() {
                "knowledge_ingest" => handle_ingest(librarian, sub).await,
                "knowledge_lint"   => handle_lint(librarian, sub).await,
                "knowledge_focus"  => handle_focus(librarian, sub).await,
                "knowledge_search" => handle_search(librarian, sub).await,
                "knowledge_get"    => handle_get(librarian, sub).await,
                other => McpResponse::err(req.id, -32601, format!("unknown tool: {other}")),
            }
        }

        // Back-compat: bare method names still work for existing callers.
        "knowledge_ingest" => handle_ingest(librarian, req).await,
        "knowledge_lint"   => handle_lint(librarian, req).await,
        "knowledge_focus"  => handle_focus(librarian, req).await,
        "knowledge_search" => handle_search(librarian, req).await,
        "knowledge_get"    => handle_get(librarian, req).await,
        method => McpResponse::err(req.id, -32601, format!("unknown method: {method}")),
    }
}

async fn handle_ingest(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    #[derive(Deserialize)]
    struct Params {
        content: String,
        #[serde(default)] source: Option<String>,
        #[serde(default)] session_id: Option<String>,
    }

    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => return McpResponse::err(req.id, -32602, format!("invalid params: {e}")),
    };

    let mut doc = RawDoc::from_path(params.source.unwrap_or_else(|| "mcp:ingest".into()), params.content);
    doc.session_id = params.session_id;

    match librarian.compile(doc).await {
        Ok(entry) => McpResponse::ok(req.id, entry_summary(&entry)),
        Err(e)    => McpResponse::err(req.id, -32000, e.to_string()),
    }
}

async fn handle_lint(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    match librarian.lint().await {
        Ok(reports) => McpResponse::ok(req.id, serde_json::json!({ "report_count": reports.len(), "reports": reports })),
        Err(e)      => McpResponse::err(req.id, -32000, e.to_string()),
    }
}

async fn handle_focus(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    #[derive(Deserialize)]
    struct Params { topic: String, #[serde(default = "default_k")] k: usize }
    fn default_k() -> usize { 10 }

    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => return McpResponse::err(req.id, -32602, format!("invalid params: {e}")),
    };

    match librarian.focus(&params.topic, params.k).await {
        Ok(mini_kb) => McpResponse::ok(req.id, serde_json::json!({ "topic": params.topic, "content": mini_kb })),
        Err(e)      => McpResponse::err(req.id, -32000, e.to_string()),
    }
}

async fn handle_search(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    #[derive(Deserialize)]
    struct Params { query: String, #[serde(default = "default_k")] k: usize }
    fn default_k() -> usize { 5 }

    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => return McpResponse::err(req.id, -32602, format!("invalid params: {e}")),
    };

    match librarian.store.search(&params.query, params.k).await {
        Ok(entries) => McpResponse::ok(req.id, serde_json::json!({
            "query": params.query,
            "results": entries.iter().map(entry_summary).collect::<Vec<_>>()
        })),
        Err(e) => McpResponse::err(req.id, -32000, e.to_string()),
    }
}

async fn handle_get(librarian: &Arc<Librarian>, req: McpRequest) -> McpResponse {
    #[derive(Deserialize)]
    struct Params { id: String }

    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => return McpResponse::err(req.id, -32602, format!("invalid params: {e}")),
    };

    match librarian.store.get(&ArticleId::from(params.id)).await {
        Ok(entry) => McpResponse::ok(req.id, entry),
        Err(e)    => McpResponse::err(req.id, -32000, e.to_string()),
    }
}

fn entry_summary(e: &WikiEntry) -> serde_json::Value {
    serde_json::json!({ "id": e.id, "title": e.title, "tags": e.tags, "revision": e.revision, "updated_at": e.updated_at })
}
