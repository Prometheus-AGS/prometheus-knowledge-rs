use crate::registry::{AgentTool, KnowledgeTool, ToolCall, ToolResult};
use pk_core::types::RawDoc;
use pk_librarian::Librarian;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// UarToolProvider — in-process bridge
//
// Owns an Arc<Librarian> and dispatches tool calls directly.
// No HTTP round-trip. Use this when UAR and Librarian share a process.
//
// Registration example (adapt to your UAR ToolRegistry):
//   let provider = Arc::new(UarToolProvider::new(Arc::clone(&librarian)));
//   for tool in provider.tools() { registry.register(tool); }
// ---------------------------------------------------------------------------

pub struct UarToolProvider {
    librarian: Arc<Librarian>,
}

impl UarToolProvider {
    pub fn new(librarian: Arc<Librarian>) -> Self {
        Self { librarian }
    }

    pub fn tools(self: Arc<Self>) -> Vec<Box<dyn AgentTool>> {
        KnowledgeTool::all()
            .iter()
            .map(|&kind| {
                Box::new(InProcessTool { kind, provider: Arc::clone(&self) }) as Box<dyn AgentTool>
            })
            .collect()
    }
}

struct InProcessTool {
    kind: KnowledgeTool,
    provider: Arc<UarToolProvider>,
}

#[async_trait::async_trait]
impl AgentTool for InProcessTool {
    fn name(&self) -> &str { self.kind.name() }
    fn description(&self) -> &str { self.kind.description() }
    fn input_schema(&self) -> serde_json::Value { self.kind.input_schema() }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        let lib = &self.provider.librarian;
        match self.kind {
            KnowledgeTool::Ingest => exec_ingest(lib, call).await,
            KnowledgeTool::Lint   => exec_lint(lib, call).await,
            KnowledgeTool::Focus  => exec_focus(lib, call).await,
            KnowledgeTool::Search => exec_search(lib, call).await,
            KnowledgeTool::Get    => exec_get(lib, call).await,
        }
    }
}

async fn exec_ingest(lib: &Arc<Librarian>, call: ToolCall) -> ToolResult {
    let content = match call.input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c.to_owned(),
        None => return ToolResult::err(call.id, "missing required param: content"),
    };
    let source = call.input.get("source").and_then(|v| v.as_str())
        .unwrap_or("uar:ingest").to_owned();

    let mut doc = RawDoc::from_path(source, content);
    doc.session_id = call.input.get("session_id").and_then(|v| v.as_str()).map(str::to_owned);

    match lib.compile(doc).await {
        Ok(entry) => ToolResult::ok(call.id, format!(
            "Compiled → **{}** `[{}]`\ntags: {}", entry.title, entry.id, entry.tags.join(", ")
        )),
        Err(e) => ToolResult::err(call.id, e.to_string()),
    }
}

async fn exec_lint(lib: &Arc<Librarian>, call: ToolCall) -> ToolResult {
    match lib.lint().await {
        Ok(reports) if reports.is_empty() => {
            ToolResult::ok(call.id, "✓ Knowledge base is clean — no issues found.")
        }
        Ok(reports) => {
            let mut out = format!("Found {} issue(s):\n\n", reports.len());
            for r in &reports {
                let icon = match r.severity {
                    pk_core::types::LintSeverity::Error   => "✗",
                    pk_core::types::LintSeverity::Warning => "⚠",
                    pk_core::types::LintSeverity::Info    => "ℹ",
                };
                let entry = r.entry_id.as_ref().map(|id| id.as_str().to_owned())
                    .unwrap_or_else(|| "(global)".into());
                out.push_str(&format!("{icon} `{entry}` — {}\n  → {}\n\n", r.issue, r.suggestion));
            }
            ToolResult::ok(call.id, out)
        }
        Err(e) => ToolResult::err(call.id, e.to_string()),
    }
}

async fn exec_focus(lib: &Arc<Librarian>, call: ToolCall) -> ToolResult {
    let topic = match call.input.get("topic").and_then(|v| v.as_str()) {
        Some(t) => t.to_owned(),
        None => return ToolResult::err(call.id, "missing required param: topic"),
    };
    let k = call.input.get("k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    match lib.focus(&topic, k).await {
        Ok(mini_kb) => ToolResult::ok(call.id, mini_kb),
        Err(e)      => ToolResult::err(call.id, e.to_string()),
    }
}

async fn exec_search(lib: &Arc<Librarian>, call: ToolCall) -> ToolResult {
    let query = match call.input.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.to_owned(),
        None => return ToolResult::err(call.id, "missing required param: query"),
    };
    let k = call.input.get("k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

    match lib.store.search(&query, k).await {
        Ok(entries) if entries.is_empty() => {
            ToolResult::ok(call.id, format!("No results for: `{query}`"))
        }
        Ok(entries) => {
            let mut out = format!("Search results for `{query}`:\n\n");
            for e in &entries {
                out.push_str(&format!("- **{}** `[{}]` — tags: {}\n", e.title, e.id, e.tags.join(", ")));
            }
            ToolResult::ok(call.id, out)
        }
        Err(e) => ToolResult::err(call.id, e.to_string()),
    }
}

async fn exec_get(lib: &Arc<Librarian>, call: ToolCall) -> ToolResult {
    let id = match call.input.get("id").and_then(|v| v.as_str()) {
        Some(i) => pk_core::types::ArticleId::from(i),
        None    => return ToolResult::err(call.id, "missing required param: id"),
    };

    match lib.store.get(&id).await {
        Ok(entry) => ToolResult::ok(call.id, format!(
            "# {}\n\n**ID:** `{}`  \n**Tags:** {}  \n**Revision:** {}  \n\n---\n\n{}",
            entry.title, entry.id, entry.tags.join(", "), entry.revision, entry.content
        )),
        Err(e) => ToolResult::err(call.id, e.to_string()),
    }
}
