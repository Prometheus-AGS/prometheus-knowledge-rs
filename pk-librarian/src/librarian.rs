use crate::{
    keyword_extract::extract_query,
    parse::parse_json,
    prompts::{
        compile_user_prompt, fix_user_prompt, focus_user_prompt, lint_user_prompt, COMPILE_SYSTEM,
        FIX_SYSTEM, FOCUS_SYSTEM, LINT_SYSTEM,
    },
    router::{ModelRouter, TaskKind},
};
use pk_core::{
    error::{PkError, PkResult},
    types::{ArticleId, LintReport, LintSeverity, RawDoc, Source, WikiEntry},
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
        Self {
            store,
            router: Arc::new(router),
            event_tx,
        }
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

        let mut entry = parse_compile_response(&response)?;

        // Ingest-time duplicate detection: the LLM synthesizes a fresh title
        // (and therefore a fresh ArticleId) on every call, even over content
        // that repeats — so identity has to be anchored to content, not the
        // title wording (GitHub issue #7). Exact repeats are caught by a
        // normalized content hash; near-duplicates fall back to a
        // word-overlap check against the related-entries candidates already
        // fetched above.
        let content_hash = pk_store::normalized_content_hash(&raw.content);
        let duplicate_of = match self.store.find_by_content_hash(&content_hash).await {
            Some(id) => Some(id),
            None => pk_store::find_near_duplicate(&raw.content, &related),
        };
        if let Some(existing_id) = duplicate_of {
            entry.id = existing_id;
        }
        pk_store::stamp_content_hash(&mut entry, &content_hash);

        let entry = self.store.upsert(entry).await?;
        // upsert() bumps revision only when the id already existed in the
        // store, so checking the *returned* entry's revision (not the
        // pre-upsert value, which WikiEntry::new always sets to 0) correctly
        // drives the Creation vs Update distinction in the OKF log — including
        // when the id was reused via duplicate-content detection above rather
        // than an incidental title-slug collision.
        let is_new = entry.revision == 0;

        // Maintain the two OKF reserved bundle files (§8 index, §9 log) after
        // every ingest. Best-effort: a bookkeeping failure must not lose the
        // compiled entry, which is already persisted.
        if let Err(e) = self.store.regenerate_index().await {
            error!(err = %e, "failed to regenerate index.md");
        }
        let action = if is_new { "Creation" } else { "Update" };
        if let Err(e) = self.store.append_log(action, &entry.title, &entry.id).await {
            error!(err = %e, "failed to append log.md");
        }

        let _ = self.event_tx.send(LibrarianEvent::compiled(
            entry.id.clone(),
            entry.title.clone(),
            entry.tags.clone(),
        ));

        info!(id = %entry.id, title = %entry.title, "compiled entry");
        Ok(entry)
    }

    pub async fn lint(&self) -> PkResult<Vec<LintReport>> {
        self.lint_with_options(true, 50).await
    }

    /// Run deterministic conformance checks and, when requested, semantic
    /// checks in bounded batches. Batching prevents large shared stores from
    /// exceeding model-server request limits while keeping legacy `lint()`
    /// behavior intact.
    pub async fn lint_with_options(
        &self,
        include_semantic: bool,
        semantic_batch_size: usize,
    ) -> PkResult<Vec<LintReport>> {
        let snapshot = self.store.snapshot().await?;
        let count = snapshot.len();
        info!(entries = count, "starting lint pass");

        // Deterministic OKF v0.2 §11 conformance always runs first — it needs no
        // model, so it stays reliable even when the lint LLM is unavailable.
        let mut reports = self.store.okf_conformance_reports().await?;
        let okf_count = reports.len();

        // LLM content-quality lint (missing links, staleness, duplicates, …)
        // is best-effort: a lint-model failure must not suppress the
        // conformance results, so on error we log and return what we have.
        if include_semantic && !snapshot.is_empty() {
            let batch_size = semantic_batch_size.clamp(1, 100);
            for (batch_index, batch) in snapshot.chunks(batch_size).enumerate() {
                match serde_json::to_string(batch) {
                    Ok(snapshot_json) => {
                        let client = self.router.build_client(TaskKind::Lint);
                        match client
                            .complete(LINT_SYSTEM, &lint_user_prompt(&snapshot_json), None, 0.1)
                            .await
                        {
                            Ok(response) => match parse_lint_response(&response) {
                                Ok(llm_reports) => reports.extend(llm_reports),
                                Err(e) => {
                                    error!(batch = batch_index, err = %e, "lint LLM response parse failed; keeping completed reports");
                                    reports.push(semantic_batch_failure(batch_index, &e));
                                }
                            },
                            Err(e) => {
                                let error = PkError::llm(e.to_string());
                                error!(batch = batch_index, err = %error, "lint LLM call failed; keeping completed reports");
                                reports.push(semantic_batch_failure(batch_index, &error));
                            }
                        }
                    }
                    Err(e) => {
                        let error = PkError::llm(e.to_string());
                        error!(batch = batch_index, err = %error, "snapshot serialization failed; skipping batch");
                        reports.push(semantic_batch_failure(batch_index, &error));
                    }
                }
            }
        }

        info!(
            issues = reports.len(),
            okf_conformance = okf_count,
            "lint pass complete"
        );
        let _ = self
            .event_tx
            .send(LibrarianEvent::lint_completed(reports.clone(), count));
        Ok(reports)
    }

    pub async fn focus(&self, topic: &str, k: usize) -> PkResult<String> {
        // SP-002: sliding-window extraction for long prompts (> 2000 chars)
        let search_query = extract_query(topic);
        let candidates = self.store.search(&search_query, k).await?;
        info!(
            topic_len = topic.len(),
            query_len = search_query.len(),
            candidates = candidates.len(),
            "building focus brief"
        );

        if candidates.is_empty() {
            return Ok(format!(
                "# {topic}\n\nNo matching articles found in the knowledge base."
            ));
        }

        let candidates_md = entries_to_context_str(&candidates);
        let client = self.router.build_client(TaskKind::Focus);
        let response = client
            .complete(
                FOCUS_SYSTEM,
                &focus_user_prompt(topic, &candidates_md),
                None,
                0.3,
            )
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

        // Deterministic OKF structural fix: an entry missing its required
        // `type` is repaired by assigning the default — no LLM needed. Keyed
        // on the stored entry's actual state, not the report text, so it is
        // robust. Falls through to the content fixer when nothing to repair.
        if let Some(fixed) = self.store.okf_autofix_type(entry_id).await? {
            let _ = self.event_tx.send(LibrarianEvent::Updated {
                entry_id: fixed.id.clone(),
                revision: fixed.revision,
                ts: chrono::Utc::now(),
            });
            return Ok(fixed);
        }

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
                        let _ = librarian
                            .event_tx
                            .send(LibrarianEvent::error(e.to_string()));
                    }
                }
            });
        }
        info!("inbox loop ended");
    }
}

fn semantic_batch_failure(batch_index: usize, error: &PkError) -> LintReport {
    LintReport {
        entry_id: None,
        severity: LintSeverity::Warning,
        issue: format!(
            "semantic lint batch {} did not produce a usable report",
            batch_index + 1
        ),
        suggestion: format!("retry this bounded semantic batch; diagnostic: {error}"),
        auto_fixable: false,
    }
}

/// Whether `label` can be written as a markdown footnote label, `[^label]`.
fn is_footnote_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Give every source an id that is unique within the entry (OKF v0.2 §5.1: the
/// footnote join is by label, so a duplicate misattributes silently).
///
/// The model discovers the sources and cites with labels of its own choosing,
/// so a usable label is kept exactly as given — re-deriving it would leave the
/// footnote in the body matching nothing. Labels are claimed in three ordered
/// passes, because anything pk invents must never take a label the model
/// chose, wherever in the list that label appears:
///
/// 1. the first occurrence of every distinct label the model chose;
/// 2. repeats of a label, which get `-2`, `-3`…;
/// 3. labels derived from `resource`, for a source with none or an unusable one.
///
/// Doing 1 and 2 together let a suffix take a label cited further down (`x`,
/// `x`, `x-2`: the second `x` became `x-2`), and doing 3 before 1 let a
/// derived label do the same. Labels compare case-folded: markdown resolves
/// `[^Doc]` against `[^doc]:`.
fn with_unique_ids(sources: Vec<Source>) -> Vec<Source> {
    use std::collections::HashSet;

    fn claim_free(base: &str, taken: &mut HashSet<String>) -> String {
        let mut n = 1_u32;
        loop {
            let candidate = if n == 1 {
                base.to_owned()
            } else {
                format!("{base}-{n}")
            };
            if taken.insert(candidate.to_lowercase()) {
                return candidate;
            }
            n += 1;
        }
    }

    let chosen: Vec<Option<&str>> = sources
        .iter()
        .map(|source| {
            source
                .id
                .as_deref()
                .filter(|label| is_footnote_label(label))
        })
        .collect();
    let mut taken: HashSet<String> = HashSet::new();
    let mut ids: Vec<Option<String>> = vec![None; sources.len()];

    for (slot, label) in ids.iter_mut().zip(&chosen) {
        if let Some(label) = label {
            if taken.insert(label.to_lowercase()) {
                *slot = Some((*label).to_owned());
            }
        }
    }
    for (slot, label) in ids.iter_mut().zip(&chosen) {
        if let (None, Some(label)) = (&slot, label) {
            *slot = Some(claim_free(label, &mut taken));
        }
    }
    for (slot, source) in ids.iter_mut().zip(&sources) {
        if slot.is_none() {
            let derived = ArticleId::from_slug(&source.resource).0;
            let base = if is_footnote_label(&derived) {
                derived.as_str()
            } else {
                "source"
            };
            *slot = Some(claim_free(base, &mut taken));
        }
    }

    sources
        .into_iter()
        .zip(ids)
        .map(|(source, id)| Source { id, ..source })
        .collect()
}

fn parse_compile_response(raw: &str) -> PkResult<WikiEntry> {
    #[derive(serde::Deserialize)]
    struct CompileOutput {
        title: String,
        content: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        // A mapping `{id, resource}` as the prompt asks, or a bare string from a
        // model that ignored it: `Source` reads both.
        #[serde(default)]
        sources: Vec<Source>,
    }

    let out: CompileOutput = parse_json(raw).map_err(|e| PkError::llm(e.to_string()))?;

    let entry = WikiEntry::new(out.title, out.content)
        .with_tags(out.tags)
        .with_sources(with_unique_ids(out.sources));

    let mut entry = entry;
    // Link graph is derived from bundle-relative links the model embedded in
    // the body (OKF v0.2 §6); the JSON `links` array is honored only for
    // back-compat with older prompts. Body links win and lead.
    let mut links = pk_store::bundle::extract_body_links(&entry.content);
    for slug in out.links {
        let id = ArticleId::from(slug);
        if !links.contains(&id) {
            links.push(id);
        }
    }
    entry.links = links;
    // OKF v0.2 §4.1: `type` is the format's one required frontmatter key.
    // The compile prompt doesn't yet ask the model to classify entries, so
    // every Librarian-compiled entry defaults to the generic OKF type.
    entry.entry_type = Some("Reference".to_string());

    Ok(entry)
}

fn parse_lint_response(raw: &str) -> PkResult<Vec<LintReport>> {
    #[derive(serde::Deserialize)]
    struct RawReport {
        entry_id: Option<String>,
        severity: String,
        issue: String,
        suggestion: String,
        #[serde(default)]
        auto_fixable: bool,
    }

    let raw_reports: Vec<RawReport> = parse_json(raw).map_err(|e| PkError::llm(e.to_string()))?;

    let reports = raw_reports
        .into_iter()
        .map(|r| LintReport {
            entry_id: r.entry_id.map(ArticleId::from),
            severity: match r.severity.as_str() {
                "error" => LintSeverity::Error,
                "warning" => LintSeverity::Warning,
                _ => LintSeverity::Info,
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
            if e.content.len() > 1200 {
                &e.content[..1200]
            } else {
                &e.content
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_entries_default_to_reference_type() {
        let response = r#"{"title":"Test","content":"body","tags":[],"links":[],"sources":[]}"#;
        let entry = parse_compile_response(response).unwrap();
        assert_eq!(entry.entry_type.as_deref(), Some("Reference"));
    }

    /// Every `[^label]` used in a body, in order of first use.
    fn footnote_labels(body: &str) -> Vec<String> {
        let mut labels = Vec::new();
        let mut rest = body;
        while let Some(start) = rest.find("[^") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(']') else { break };
            let label = after[..end].to_owned();
            if !labels.contains(&label) {
                labels.push(label);
            }
            rest = &after[end..];
        }
        labels
    }

    // OKF v0.2 §13.1 supersedes the body `# Citations` list; §5.1 attributes
    // claims with footnotes whose label is a `sources[].id`.
    #[test]
    fn the_compile_prompt_asks_for_keyed_footnotes_not_a_citations_section() {
        assert!(!crate::prompts::COMPILE_SYSTEM.contains("Citations"));
        assert!(crate::prompts::COMPILE_SYSTEM.contains("[^"));
    }

    // The model discovers the sources, so it chooses the labels it cites with.
    // Re-deriving a label it already used would leave the footnote pointing at
    // nothing: the silent misattribution §5.1 warns about.
    #[test]
    fn a_label_the_model_chose_is_kept_so_its_footnote_still_matches() {
        let response = r#"{"title":"T","content":"Sharded daily.[^ga4_schema]\n\n[^ga4_schema]: GA4 export schema","sources":[{"id":"ga4_schema","resource":"https://example.com/schema"}]}"#;

        let entry = parse_compile_response(response).unwrap();

        assert_eq!(entry.sources[0].id.as_deref(), Some("ga4_schema"));
        assert_eq!(footnote_labels(&entry.content), vec!["ga4_schema"]);
    }

    #[test]
    fn a_source_given_as_a_bare_string_gets_a_derived_id() {
        let response = r#"{"title":"T","content":"body","sources":["session:ABC 123"]}"#;

        let entry = parse_compile_response(response).unwrap();

        assert_eq!(entry.sources[0].resource, "session:ABC 123");
        assert_eq!(entry.sources[0].id.as_deref(), Some("session-abc-123"));
    }

    #[test]
    fn a_label_that_cannot_be_a_footnote_is_replaced_by_a_derived_one() {
        let response = r#"{"title":"T","content":"body","sources":[{"id":"has space]","resource":"notes/a.md"}]}"#;

        let entry = parse_compile_response(response).unwrap();

        assert_eq!(entry.sources[0].id.as_deref(), Some("notes-a-md"));
    }

    // A duplicate label misattributes silently, because the footnote join is by label.
    #[test]
    fn sources_that_reduce_to_one_label_get_distinct_ids() {
        let response = r#"{"title":"T","content":"One.[^notes-a-md]\n\n[^notes-a-md]: first","sources":["notes/a.md","notes-a.md","notes a md"]}"#;

        let entry = parse_compile_response(response).unwrap();
        let ids: Vec<_> = entry
            .sources
            .iter()
            .map(|s| s.id.clone().unwrap())
            .collect();

        assert_eq!(ids, vec!["notes-a-md", "notes-a-md-2", "notes-a-md-3"]);
        for label in footnote_labels(&entry.content) {
            assert_eq!(ids.iter().filter(|id| **id == label).count(), 1, "{label}");
        }
    }

    // Order must not decide attribution: a label derived for an uncited source
    // may not take the one the model chose, and cited with, further down the list.
    #[test]
    fn a_derived_label_never_displaces_one_the_model_cited_with() {
        let response = r#"{"title":"T","content":"Claim.[^x]\n\n[^x]: the cited one","sources":["x",{"id":"x","resource":"the-cited-source"}]}"#;

        let entry = parse_compile_response(response).unwrap();
        let cited = entry
            .sources
            .iter()
            .find(|s| s.id.as_deref() == Some("x"))
            .unwrap();

        assert_eq!(cited.resource, "the-cited-source");
        assert_eq!(entry.sources[0].id.as_deref(), Some("x-2"));
    }

    // The first fix protected a model's label from a DERIVED one. A suffix pk
    // hands out itself is the same threat: with x, x, x-2 the second x took
    // "x-2", and the body's [^x-2] - which meant the third source - resolved
    // to the second.
    #[test]
    fn a_deduplication_suffix_never_displaces_a_label_the_model_cited_with() {
        let response = r#"{"title":"T","content":"Claim.[^x-2]\n\n[^x-2]: the cited one","sources":[{"id":"x","resource":"A"},{"id":"x","resource":"B"},{"id":"x-2","resource":"the-cited-source"}]}"#;

        let entry = parse_compile_response(response).unwrap();
        let ids: Vec<_> = entry
            .sources
            .iter()
            .map(|s| s.id.clone().unwrap())
            .collect();
        let cited = entry
            .sources
            .iter()
            .find(|s| s.id.as_deref() == Some("x-2"))
            .unwrap();

        assert_eq!(cited.resource, "the-cited-source");
        assert_eq!(ids, vec!["x", "x-3", "x-2"]);
    }

    // Markdown footnote labels are case-insensitive: [^Doc] resolves [^doc]:.
    #[test]
    fn labels_that_differ_only_by_case_are_the_same_label() {
        let response = r#"{"title":"T","content":"body","sources":[{"id":"Doc","resource":"A"},{"id":"doc","resource":"B"}]}"#;

        let entry = parse_compile_response(response).unwrap();
        let ids: Vec<_> = entry
            .sources
            .iter()
            .map(|s| s.id.clone().unwrap())
            .collect();

        assert_eq!(ids, vec!["Doc", "doc-2"]);
    }

    // The suffixed candidate is compared case-folded too: with doc-2, Doc, Doc the
    // repeat may be neither "Doc" again nor "Doc-2", which is "doc-2" to markdown.
    #[test]
    fn a_suffixed_label_is_compared_case_folded_as_well() {
        let response = r#"{"title":"T","content":"body","sources":[{"id":"doc-2","resource":"A"},{"id":"Doc","resource":"B"},{"id":"Doc","resource":"C"}]}"#;

        let entry = parse_compile_response(response).unwrap();
        let ids: Vec<_> = entry
            .sources
            .iter()
            .map(|s| s.id.clone().unwrap())
            .collect();

        assert_eq!(ids, vec!["doc-2", "Doc", "Doc-3"]);
    }

    #[test]
    fn a_source_with_nothing_to_derive_a_label_from_still_gets_one() {
        let response = r#"{"title":"T","content":"body","sources":["///"]}"#;

        let entry = parse_compile_response(response).unwrap();

        assert_eq!(entry.sources[0].id.as_deref(), Some("source"));
    }
}
