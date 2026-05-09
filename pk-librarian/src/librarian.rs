use crate::{
    keyword_extract::extract_query,
    prompts::{
        compile_user_prompt, fix_user_prompt, focus_user_prompt, lint_user_prompt,
        COMPILE_SYSTEM, FIX_SYSTEM, FOCUS_SYSTEM, LINT_SYSTEM,
    },
    router::{ModelRouter, TaskKind},
};
use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, LintReport, LintSeverity, RawDoc, WikiEntry},
    LibrarianEvent,
};
use pk_store::MarkdownStore;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

pub struct Librarian {
    pub store: Arc<MarkdownStore>,
    router: Arc<ModelRouter>,
    pub event_tx: broadcast::Sender<LibrarianEvent>,
}

impl Librarian {
    pub fn new(
        store: Arc<MarkdownStore>,
        router: ModelRouter,
        event_tx: broadcast::Sender<LibrarianEvent>,
    ) -> Self {
        Self { store, router: Arc::new(router), event_tx }
    }

    pub async fn compile(&self, raw: RawDoc) -> PkResult<WikiEntry> {
        info!(source = %raw.source_path, "compiling raw doc");

        let related = self.store.related_entries(&raw, 5).await?;
        let related_md = entries_to_context_str(&related);

        let client = self.router.build_client(TaskKind::Compile);
        let user_prompt = compile_user_prompt(&raw.content, &raw.source_path, &related_md);

        let response = client
            .complete(COMPILE_SYSTEM, &user_prompt, None, 0.2)
            .await
            .map_err(|e| PkError::llm(e.to_string()))?;

        let entry = parse_compile_response(&response)?;
        let entry = self.store.upsert(entry).await?;

        let _ = self.event_tx.send(LibrarianEvent::compiled(
            entry.id.clone(),
            entry.title.clone(),
            entry.tags.clone(),
        ));

        info!(id = %entry.id, title = %entry.title, "compiled entry");
        Ok(entry)
    }

    pub async fn lint(&self) -> PkResult<Vec<LintReport>> {
        let snapshot = self.store.snapshot().await?;
        let count = snapshot.len();
        info!(entries = count, "starting lint pass");

        if snapshot.is_empty() {
            return Ok(Vec::new());
        }

        let snapshot_json = serde_json::to_string(&snapshot)
            .map_err(|e| PkError::llm(format!("snapshot serialization: {e}")))?;

        let client = self.router.build_client(TaskKind::Lint);
        let response = client
            .complete(LINT_SYSTEM, &lint_user_prompt(&snapshot_json), None, 0.1)
            .await
            .map_err(|e| PkError::llm(e.to_string()))?;

        let reports = parse_lint_response(&response)?;
        info!(issues = reports.len(), "lint pass complete");

        let _ = self.event_tx.send(LibrarianEvent::lint_completed(reports.clone(), count));
        Ok(reports)
    }

    pub async fn focus(&self, topic: &str, k: usize) -> PkResult<String> {
        // SP-002: sliding-window extraction for long prompts (> 2000 chars)
        let search_query = extract_query(topic);
        let candidates = self.store.search(&search_query, k).await?;
        info!(topic_len = topic.len(), query_len = search_query.len(), candidates = candidates.len(), "building focus brief");

        if candidates.is_empty() {
            return Ok(format!("# {topic}\n\nNo matching articles found in the knowledge base."));
        }

        let candidates_md = entries_to_context_str(&candidates);
        let client = self.router.build_client(TaskKind::Focus);
        let response = client
            .complete(FOCUS_SYSTEM, &focus_user_prompt(topic, &candidates_md), None, 0.3)
            .await
            .map_err(|e| PkError::llm(e.to_string()))?;

        let _ = self.event_tx.send(LibrarianEvent::Focused {
            topic: topic.to_owned(),
            entry_count: candidates.len(),
            ts: chrono::Utc::now(),
        });

        Ok(response)
    }

    pub async fn auto_fix(&self, report: &LintReport) -> PkResult<WikiEntry> {
        let entry_id = report
            .entry_id
            .as_ref()
            .ok_or_else(|| PkError::llm("auto_fix requires an entry_id"))?;

        let mut entry = self.store.get(entry_id).await?;
        let client = self.router.build_client(TaskKind::Fix);

        let new_content = client
            .complete(
                FIX_SYSTEM,
                &fix_user_prompt(&entry.content, &report.issue, &report.suggestion),
                None,
                0.1,
            )
            .await
            .map_err(|e| PkError::llm(e.to_string()))?;

        entry.content = new_content;
        let entry = self.store.upsert(entry).await?;

        let _ = self.event_tx.send(LibrarianEvent::Updated {
            entry_id: entry.id.clone(),
            revision: entry.revision,
            ts: chrono::Utc::now(),
        });

        Ok(entry)
    }

    pub async fn run_inbox_loop(self: Arc<Self>, mut raw_rx: mpsc::Receiver<RawDoc>) {
        info!("inbox loop started");
        while let Some(doc) = raw_rx.recv().await {
            let librarian = Arc::clone(&self);
            tokio::spawn(async move {
                match librarian.compile(doc).await {
                    Ok(entry) => info!(id = %entry.id, "inbox: compiled"),
                    Err(e) => {
                        error!(err = %e, "inbox: compile failed");
                        let _ = librarian.event_tx.send(LibrarianEvent::error(e.to_string()));
                    }
                }
            });
        }
        info!("inbox loop ended");
    }
}

fn strip_json_fences(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

fn parse_compile_response(raw: &str) -> PkResult<WikiEntry> {
    let json_str = strip_json_fences(raw);

    #[derive(serde::Deserialize)]
    struct CompileOutput {
        title: String,
        content: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        #[serde(default)]
        sources: Vec<String>,
    }

    let out: CompileOutput = serde_json::from_str(json_str)
        .map_err(|e| PkError::llm(format!("compile response parse: {e}\nraw: {json_str}")))?;

    let entry = WikiEntry::new(out.title, out.content)
        .with_tags(out.tags)
        .with_sources(out.sources);

    let mut entry = entry;
    entry.links = out.links.into_iter().map(ArticleId::from).collect();

    Ok(entry)
}

fn parse_lint_response(raw: &str) -> PkResult<Vec<LintReport>> {
    let json_str = strip_json_fences(raw);

    #[derive(serde::Deserialize)]
    struct RawReport {
        entry_id: Option<String>,
        severity: String,
        issue: String,
        suggestion: String,
        #[serde(default)]
        auto_fixable: bool,
    }

    let raw_reports: Vec<RawReport> = serde_json::from_str(json_str)
        .map_err(|e| PkError::llm(format!("lint response parse: {e}\nraw: {json_str}")))?;

    let reports = raw_reports
        .into_iter()
        .map(|r| LintReport {
            entry_id: r.entry_id.map(ArticleId::from),
            severity: match r.severity.as_str() {
                "error"   => LintSeverity::Error,
                "warning" => LintSeverity::Warning,
                _         => LintSeverity::Info,
            },
            issue: r.issue,
            suggestion: r.suggestion,
            auto_fixable: r.auto_fixable,
        })
        .collect();

    Ok(reports)
}

fn entries_to_context_str(entries: &[WikiEntry]) -> String {
    if entries.is_empty() {
        return "(none)".to_owned();
    }

    let mut out = String::with_capacity(entries.len() * 512);
    for e in entries {
        out.push_str(&format!(
            "## {} [{}]\ntags: {}\n\n{}\n\n---\n",
            e.title,
            e.id,
            e.tags.join(", "),
            if e.content.len() > 1200 { &e.content[..1200] } else { &e.content }
        ));
    }
    out
}
