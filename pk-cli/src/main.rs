#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: Jemalloc = Jemalloc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use pk_core::types::RawDoc;
use pk_event_store::EventStore;
use pk_librarian::{Librarian, ModelRouter};
use pk_store::{commit_prompt_snapshot, read_prompt_snapshot, MarkdownStore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::broadcast;

/// KB scope: project-local (default) or globally shared across projects.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum KbScope {
    /// Write to <project_root>/.prometheus/knowledge/ (default)
    Project,
    /// Write to ~/.prometheus/knowledge/shared/ (cross-project patterns)
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
enum ContextScope {
    Project,
    Shared,
    Global,
}

impl ContextScope {
    fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Shared => "shared",
            Self::Global => "global",
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Project => 0,
            Self::Shared => 1,
            Self::Global => 2,
        }
    }
}

impl KbScope {
    fn snapshot_label(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ContextFormat {
    Hook,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "pk", version, about = "Prometheus Knowledge CLI")]
struct Cli {
    /// Override KB directory. If unset, resolved automatically:
    ///   - inside a project root → <project_root>/.prometheus/knowledge/
    ///   - outside any project root → ~/.prometheus/knowledge/
    #[arg(long, env = "PK_KB_DIR", global = true)]
    kb_dir: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Ingest a file or stdin into the knowledge base
    Ingest {
        #[arg()]
        file: Option<PathBuf>,
        #[arg(long)]
        source: Option<String>,
        /// KB scope: project (default) writes to project-local KB;
        /// shared writes to ~/.prometheus/knowledge/shared/
        #[arg(long, value_enum, default_value = "project")]
        scope: KbScope,
        /// Skip confirmation prompt when using --scope=shared
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Run a lint pass over the full knowledge base
    Lint {
        #[arg(long, default_value_t = false)]
        fix: bool,
        /// Run deterministic schema and parse checks only; never call an LLM.
        #[arg(long, default_value_t = false)]
        mechanical_only: bool,
        /// Emit a machine-readable report.
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Exit nonzero when any required-schema or parse error remains.
        #[arg(long, default_value_t = false)]
        strict_errors: bool,
        /// Maximum entries sent in one semantic lint request.
        #[arg(long, default_value_t = 50)]
        semantic_batch_size: usize,
    },
    /// Build a focused mini-KB for a topic and print to stdout
    Focus {
        #[arg()]
        topic: String,
        #[arg(long, default_value_t = 10)]
        k: usize,
        /// Treat the topic as N prior turns (newline-separated); later turns
        /// reinforce keywords while earlier turns decay. SP-004.
        #[arg(long, value_name = "N")]
        context_window: Option<usize>,
        /// Skip cache lookup and write; always call the full focus pipeline. SP-003.
        #[arg(long, default_value_t = false)]
        no_cache: bool,
        /// Wrap output as a system-context block for injection into AI prompts. SP-005.
        /// Outputs: <system-context>\n{result}\n</system-context>
        #[arg(long, default_value_t = false)]
        inject_as_system_context: bool,
    },
    /// Retrieve bounded local context without invoking an LLM
    Context {
        #[arg()]
        query: String,
        /// Knowledge scopes to search. Repeat for multiple scopes; defaults to all three.
        #[arg(long = "scope", value_enum)]
        scopes: Vec<ContextScope>,
        #[arg(long, default_value_t = 8)]
        limit: usize,
        /// Maximum immutable snapshot candidates inspected across all scopes.
        #[arg(long, default_value_t = 128)]
        max_candidates: usize,
        /// Maximum bytes emitted in hook format.
        #[arg(long, default_value_t = 6_000)]
        max_bytes: usize,
        #[arg(long, value_enum, default_value = "hook")]
        format: ContextFormat,
    },
    /// Publish immutable prompt-snapshot generations from local knowledge stores.
    Snapshot {
        /// Knowledge scopes to publish. Repeat for multiple scopes; defaults to all three.
        #[arg(long = "scope", value_enum)]
        scopes: Vec<ContextScope>,
    },
    /// Full-text search the knowledge base
    Search {
        #[arg()]
        query: String,
        #[arg(long, default_value_t = 5)]
        k: usize,
    },
    /// Print a single wiki entry by ID
    Get {
        #[arg()]
        id: String,
    },
    /// List all wiki entries
    List,
    /// Dump knowledge base stats
    Stats,
    /// Initialize a new project with Prometheus conventions
    Init {
        /// Project name (defaults to current directory name)
        #[arg(long)]
        name: Option<String>,
        /// Technology stack descriptor (e.g. "rust", "typescript", "python")
        #[arg(long)]
        stack: Option<String>,
        /// Skip confirmation prompt
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Check Prometheus setup health (hooks, sycophancy binary, KB scoping)
    Doctor {
        /// Output as JSON instead of human-readable report
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Migrate global KB entries to per-project directories (dry-run by default)
    MigrateToPerProject {
        /// Execute the migration (default is dry-run; review output first)
        #[arg(long, default_value_t = false)]
        execute: bool,
    },
    /// BDD codegraph operations (scenario → source file mapping)
    Codegraph {
        #[command(subcommand)]
        action: CodegraphCmd,
    },
    /// LibrarianEvent query and inspection (SP-019)
    Events {
        #[command(subcommand)]
        action: EventsCmd,
    },
    /// Migrate single-store events to dual-store (KG + episodic) layout (SP-020)
    MigrateStores {
        /// Print migration plan without applying changes (default: true)
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        /// Apply the migration (reshards .prometheus/events.jsonl into kg + episodic shards)
        #[arg(long, default_value_t = false)]
        execute: bool,
    },
}

#[derive(Debug, Subcommand)]
enum EventsCmd {
    /// List recent events for this project
    List {
        /// Filter by event kind (compiled, lint_completed, focused, updated, etc.)
        #[arg(long)]
        kind: Option<String>,
        /// Max events to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Output as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show all events that affected a specific entry
    ForEntry {
        #[arg()]
        entry_id: String,
        /// Output as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CodegraphCmd {
    /// Extract static codegraph from feature files and TypeScript source
    Extract {
        /// Path to the project root containing scripts/codegraph-extract.ts
        /// (defaults to cwd or detected project root)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Output path for codegraph.json (default: tests/reports/codegraph.json)
        #[arg(long)]
        output: Option<PathBuf>,
        /// CI mode: exit 1 if no scenarios extracted
        #[arg(long, default_value_t = false)]
        ci: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();

    // Resolve KB directory: explicit flag > env > project-root > global fallback
    let kb_dir = resolve_kb_dir(cli.kb_dir.as_deref(), &cli.command);

    // For migrate command we don't open the store at the resolved path yet
    if let Cmd::MigrateToPerProject { execute } = &cli.command {
        return run_migrate(*execute).await;
    }

    if let Cmd::Context {
        query,
        scopes,
        limit,
        max_candidates,
        max_bytes,
        format,
    } = &cli.command
    {
        return run_context(
            query,
            scopes,
            *limit,
            *max_candidates,
            *max_bytes,
            *format,
            cli.kb_dir.as_deref(),
        )
        .await;
    }
    if let Cmd::Snapshot { scopes } = &cli.command {
        return run_snapshot(scopes, cli.kb_dir.as_deref()).await;
    }
    if let Cmd::Doctor { json } = &cli.command {
        return run_doctor(*json, &kb_dir);
    }

    let store = Arc::new(MarkdownStore::open(&kb_dir).await?);
    let (event_tx, event_rx) = broadcast::channel(64);
    let librarian = Arc::new(Librarian::new(
        Arc::clone(&store),
        ModelRouter::from_env(),
        event_tx,
    ));

    // Read-only commands must not create `.prometheus/events.jsonl` in the
    // caller's current repository. Persist events only for commands that
    // intentionally produce durable knowledge events.
    let _persist_handle = if matches!(&cli.command, Cmd::Ingest { .. } | Cmd::Focus { .. }) {
        let persist_project_root =
            find_project_root().unwrap_or_else(|| std::env::current_dir().expect("cwd must exist"));
        Some(tokio::spawn(async move {
            let event_store = EventStore::for_project(&persist_project_root, "project");
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Err(e) = event_store.persist(&event).await {
                            tracing::warn!("event persist failed: {e}");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("event persist subscriber lagged by {n} messages");
                    }
                }
            }
        }))
    } else {
        None
    };

    match cli.command {
        Cmd::Ingest {
            file,
            source,
            scope,
            yes,
        } => {
            // For shared scope, require confirmation unless --yes passed
            if matches!(scope, KbScope::Shared) && !yes {
                eprint!("Writing to shared KB (~/.prometheus/knowledge/shared/). This crosses project boundaries. Continue? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let (content, source_label) = match file {
                Some(path) => {
                    let content = tokio::fs::read_to_string(&path).await?;
                    let label = source.unwrap_or_else(|| path.to_string_lossy().into_owned());
                    (content, label)
                }
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    (buf, source.unwrap_or_else(|| "stdin".into()))
                }
            };
            let doc = RawDoc::from_path(source_label, content);
            let entry = librarian.compile(doc).await?;
            commit_prompt_snapshot(&kb_dir, scope.snapshot_label(), store.snapshot().await?)?;
            println!("✓ compiled → {} [{}]", entry.title, entry.id);
        }

        Cmd::Lint {
            fix,
            mechanical_only,
            json,
            strict_errors,
            semantic_batch_size,
        } => {
            let reports = librarian
                .lint_with_options(!mechanical_only, semantic_batch_size)
                .await?;
            if reports.is_empty() {
                if json {
                    println!("{}", serde_json::json!({"issues": [], "fixed": 0}));
                } else {
                    println!("✓ no issues found");
                }
                return Ok(());
            }
            let mut fixed = 0usize;
            for report in &reports {
                if json {
                    if fix && report.auto_fixable && librarian.auto_fix(report).await.is_ok() {
                        fixed += 1;
                    }
                    continue;
                }
                let icon = match report.severity {
                    pk_core::types::LintSeverity::Error => "✗",
                    pk_core::types::LintSeverity::Warning => "⚠",
                    pk_core::types::LintSeverity::Info => "ℹ",
                };
                let entry_label = report
                    .entry_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned())
                    .unwrap_or_else(|| "(global)".into());
                println!(
                    "{icon} [{entry_label}] {} — {}",
                    report.severity, report.issue
                );
                println!("  → {}", report.suggestion);
                if fix && report.auto_fixable {
                    match librarian.auto_fix(report).await {
                        Ok(entry) => {
                            println!("  ✓ fixed → revision {}", entry.revision);
                            fixed += 1;
                        }
                        Err(e) => println!("  ✗ fix failed: {e}"),
                    }
                }
            }
            if fix && fixed > 0 {
                store.regenerate_index().await?;
            }
            if json {
                println!("{}", serde_json::json!({"issues": reports, "fixed": fixed}));
            } else {
                println!("\n{} issue(s)", reports.len());
                if fix {
                    println!("{fixed} auto-fixed");
                }
            }
            let remaining_errors = store
                .okf_conformance_reports()
                .await?
                .iter()
                .any(|report| report.severity == pk_core::types::LintSeverity::Error);
            if strict_errors && remaining_errors {
                anyhow::bail!("strict lint failed: required-schema or parse errors remain");
            }
        }

        Cmd::Focus {
            topic,
            k,
            context_window,
            no_cache,
            inject_as_system_context,
        } => {
            let effective_topic = if let Some(n_turns) = context_window {
                pk_librarian::extract_query_multi_turn(&topic, n_turns)
            } else {
                pk_librarian::extract_query(&topic)
            };

            // SP-003: SHA256-keyed result cache under ~/.prometheus/pk-focus-cache/
            let cache_key = {
                let mut h = Sha256::new();
                h.update(effective_topic.as_bytes());
                h.update(k.to_string().as_bytes());
                format!("{:x}", h.finalize())
            };
            let cache_dir = dirs::home_dir()
                .map(|h| h.join(".prometheus").join("pk-focus-cache"))
                .unwrap_or_else(|| PathBuf::from(".prometheus/pk-focus-cache"));
            let cache_file = cache_dir.join(format!("{cache_key}.md"));

            if !no_cache {
                if let Ok(cached) = tokio::fs::read_to_string(&cache_file).await {
                    if inject_as_system_context {
                        println!("<system-context>\n{cached}\n</system-context>");
                    } else {
                        print!("{cached}");
                    }
                    return Ok(());
                }
            }

            let result = librarian.focus(&effective_topic, k).await?;

            if !no_cache {
                if let Err(e) = tokio::fs::create_dir_all(&cache_dir).await {
                    tracing::warn!("pk-focus-cache dir create failed: {e}");
                } else if let Err(e) = tokio::fs::write(&cache_file, result.as_bytes()).await {
                    tracing::warn!("pk-focus-cache write failed: {e}");
                }
            }

            if inject_as_system_context {
                println!("<system-context>\n{result}\n</system-context>");
            } else {
                println!("{result}");
            }
        }

        Cmd::Context { .. } | Cmd::Snapshot { .. } => {
            unreachable!("handled before opening the default store")
        }

        Cmd::Search { query, k } => {
            let results = store.search(&query, k).await?;
            if results.is_empty() {
                println!("no results for: {query}");
            } else {
                for e in &results {
                    println!("[{}] {} — tags: {}", e.id, e.title, e.tags.join(", "));
                }
            }
        }

        Cmd::Get { id } => {
            let entry = store.get(&pk_core::types::ArticleId::from(id)).await?;
            println!(
                "# {}\n\ntags: {}\n\n{}",
                entry.title,
                entry.tags.join(", "),
                entry.content
            );
        }

        Cmd::List => {
            let entries = store.snapshot().await?;
            if entries.is_empty() {
                println!("(empty knowledge base)");
            } else {
                for e in &entries {
                    println!("[{}] {} (rev {})", e.id, e.title, e.revision);
                }
                println!("\n{} entries", entries.len());
            }
        }

        Cmd::Stats => {
            let entries = store.snapshot().await?;
            let total_tags: usize = entries.iter().map(|e| e.tags.len()).sum();
            let total_links: usize = entries.iter().map(|e| e.links.len()).sum();
            println!("entries:     {}", entries.len());
            println!("total tags:  {total_tags}");
            println!("total links: {total_links}");
            println!("kb dir:      {}", kb_dir.display());
        }

        Cmd::MigrateToPerProject { .. } => unreachable!("handled above"),

        Cmd::Codegraph { action } => match action {
            CodegraphCmd::Extract {
                project,
                output,
                ci,
            } => {
                run_codegraph_extract(project, output, ci)?;
            }
        },

        Cmd::Events { action } => {
            let project_root = find_project_root()
                .unwrap_or_else(|| std::env::current_dir().expect("cwd must exist"));
            let event_store = EventStore::for_project(&project_root, "project");

            match action {
                EventsCmd::List { kind, limit, json } => {
                    let records = event_store.list(kind.as_deref(), limit)?;
                    if records.is_empty() {
                        println!("(no events recorded for this project)");
                    } else if json {
                        println!("{}", serde_json::to_string_pretty(&records)?);
                    } else {
                        for r in &records {
                            println!(
                                "[{}] {} — {} ({})",
                                r.timestamp.format("%Y-%m-%d %H:%M:%S"),
                                r.kind,
                                if r.affects.is_empty() {
                                    "(no entry)".to_string()
                                } else {
                                    r.affects.join(", ")
                                },
                                r.id
                            );
                        }
                        println!("\n{} event(s)", records.len());
                    }
                }
                EventsCmd::ForEntry { entry_id, json } => {
                    let records = event_store.for_entry(&entry_id)?;
                    if records.is_empty() {
                        println!("(no events found for entry {entry_id})");
                    } else if json {
                        println!("{}", serde_json::to_string_pretty(&records)?);
                    } else {
                        for r in &records {
                            println!(
                                "[{}] {} — {}",
                                r.timestamp.format("%Y-%m-%d %H:%M:%S"),
                                r.kind,
                                r.id
                            );
                        }
                        println!("\n{} event(s) for {entry_id}", records.len());
                    }
                }
            }
        }
        Cmd::Init { name, stack, yes } => {
            run_init(name, stack, yes)?;
        }

        Cmd::Doctor { .. } => unreachable!("doctor returns before opening the knowledge store"),

        Cmd::MigrateStores { execute, .. } => {
            let project_root = find_project_root()
                .unwrap_or_else(|| std::env::current_dir().expect("cwd must exist"));

            let plan = pk_event_store::migrate::plan(&project_root)?;
            println!("Migration plan:");
            println!("  KG records:       {}", plan.kg_records.len());
            println!("  Episodic records: {}", plan.episodic_records.len());
            println!("  Unclassified:     {}", plan.unclassified.len());

            if !execute {
                println!("\n(dry-run) Pass --execute to apply the migration.");
                return Ok(());
            }

            pk_event_store::migrate::apply(&plan, &project_root)?;
            println!("✓ Migration applied:");
            println!("  → .prometheus/events-kg.jsonl");
            println!("  → .prometheus/events-episodic.jsonl");
            if !plan.unclassified.is_empty() {
                println!(
                    "  → .prometheus/events-unsorted.jsonl ({} records need manual triage)",
                    plan.unclassified.len()
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
struct ContextCandidate {
    scope: ContextScope,
    entry: pk_core::types::WikiEntry,
    score: f32,
}

#[derive(Debug, Serialize)]
struct ContextOutput {
    query: String,
    snapshot_generations: BTreeMap<String, String>,
    candidate_count: usize,
    byte_count: usize,
    failures: Vec<ContextFailure>,
    results: Vec<ContextItem>,
}

#[derive(Debug, Serialize)]
struct ContextFailure {
    scope: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct ContextItem {
    scope: String,
    id: String,
    title: String,
    snippet: String,
    score: f32,
}

async fn run_context(
    query: &str,
    requested_scopes: &[ContextScope],
    limit: usize,
    max_candidates: usize,
    max_bytes: usize,
    format: ContextFormat,
    explicit_project_kb: Option<&str>,
) -> Result<()> {
    let scopes = if requested_scopes.is_empty() {
        vec![
            ContextScope::Project,
            ContextScope::Shared,
            ContextScope::Global,
        ]
    } else {
        let mut seen = HashSet::new();
        requested_scopes
            .iter()
            .copied()
            .filter(|scope| seen.insert(*scope))
            .collect()
    };
    let max_candidates = max_candidates.clamp(1, 512);
    let max_bytes = max_bytes.clamp(256, 65_536);
    let candidates_per_scope = max_candidates.div_ceil(scopes.len().max(1));
    let mut failures = Vec::new();
    let mut candidates = Vec::new();
    let mut inspected_candidates = 0usize;
    let mut generations = BTreeMap::new();
    for scope in scopes {
        let Some(path) = knowledge_root_for_scope(scope, explicit_project_kb) else {
            failures.push(ContextFailure {
                scope: scope.label().to_owned(),
                error: "no project root detected".to_owned(),
            });
            continue;
        };
        let snapshot = match read_prompt_snapshot(&path, scope.label()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                failures.push(ContextFailure {
                    scope: scope.label().to_owned(),
                    error: format!(
                        "no valid committed prompt snapshot at {}: {error}",
                        path.display()
                    ),
                });
                continue;
            }
        };
        generations.insert(scope.label().to_owned(), snapshot.generation);
        let remaining = max_candidates
            .saturating_sub(inspected_candidates)
            .min(candidates_per_scope);
        let bounded_entries = snapshot
            .entries
            .into_iter()
            .take(remaining)
            .collect::<Vec<_>>();
        inspected_candidates += bounded_entries.len();
        candidates.extend(bounded_entries.into_iter().filter_map(|entry| {
            let score = snapshot_score(query, &entry);
            (score > 0.0 || query.trim().is_empty()).then_some(ContextCandidate {
                scope,
                entry,
                score,
            })
        }));
        if inspected_candidates == max_candidates {
            break;
        }
    }

    // Select a canonical copy of duplicate IDs or duplicate content. Scope
    // priority is applied before relevance so project-local knowledge wins
    // over shared/global copies of the same document.
    candidates.sort_by(|left, right| {
        left.scope
            .priority()
            .cmp(&right.scope.priority())
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.entry.id.as_str().cmp(right.entry.id.as_str()))
    });
    let mut seen_ids = HashSet::new();
    let mut seen_content = HashSet::new();
    let mut selected = Vec::new();
    for candidate in candidates {
        let mut hasher = Sha256::new();
        hasher.update(candidate.entry.title.as_bytes());
        hasher.update([0]);
        hasher.update(candidate.entry.content.as_bytes());
        let content_hash = format!("{:x}", hasher.finalize());
        if !seen_ids.insert(candidate.entry.id.as_str().to_owned())
            || !seen_content.insert(content_hash)
        {
            continue;
        }
        selected.push(candidate);
    }
    selected.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.scope.priority().cmp(&right.scope.priority()))
            .then_with(|| left.entry.id.as_str().cmp(right.entry.id.as_str()))
    });
    selected.truncate(limit.clamp(1, 32));

    let results = selected
        .into_iter()
        .map(|candidate| ContextItem {
            scope: candidate.scope.label().to_owned(),
            id: candidate.entry.id.as_str().to_owned(),
            title: candidate.entry.title,
            snippet: context_snippet(
                candidate.entry.description.as_deref(),
                &candidate.entry.content,
            ),
            score: candidate.score,
        })
        .collect();
    let mut output = ContextOutput {
        query: query.to_owned(),
        snapshot_generations: generations,
        candidate_count: inspected_candidates,
        byte_count: 0,
        failures,
        results,
    };
    output.byte_count = rendered_context(&output).len().min(max_bytes);

    match format {
        ContextFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
        ContextFormat::Hook => print_context_hook(&output, max_bytes),
    }
    Ok(())
}

fn snapshot_score(query: &str, entry: &pk_core::types::WikiEntry) -> f32 {
    let terms = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .filter(|term| term.len() >= 2)
        .map(str::to_lowercase)
        .collect::<HashSet<_>>();
    if terms.is_empty() {
        return 1.0;
    }
    let title = entry.title.to_lowercase();
    let description = entry.description.as_deref().unwrap_or("").to_lowercase();
    let content = entry.content.to_lowercase();
    terms
        .iter()
        .map(|term| {
            (title.matches(term).count() as f32 * 4.0)
                + (description.matches(term).count() as f32 * 2.0)
                + content.matches(term).count() as f32
        })
        .sum()
}

async fn run_snapshot(
    requested_scopes: &[ContextScope],
    explicit_project_kb: Option<&str>,
) -> Result<()> {
    let scopes = if requested_scopes.is_empty() {
        vec![
            ContextScope::Project,
            ContextScope::Shared,
            ContextScope::Global,
        ]
    } else {
        requested_scopes.to_vec()
    };
    let mut published = 0usize;
    for scope in scopes {
        let Some(path) = knowledge_root_for_scope(scope, explicit_project_kb) else {
            eprintln!("{}: no project root detected", scope.label());
            continue;
        };
        if !path.join("wiki").is_dir() {
            eprintln!(
                "{}: wiki store missing at {}",
                scope.label(),
                path.display()
            );
            continue;
        }
        let store = MarkdownStore::open(&path).await?;
        let report = store.readiness_report().await;
        if report.parse_failures > 0 {
            eprintln!(
                "{}: refused snapshot because {} wiki files failed parsing",
                scope.label(),
                report.parse_failures
            );
            continue;
        }
        let snapshot = commit_prompt_snapshot(&path, scope.label(), store.snapshot().await?)?;
        println!(
            "{} {} candidates {} bytes {}",
            scope.label(),
            snapshot.candidate_count,
            snapshot.byte_count,
            snapshot.generation
        );
        published += 1;
    }
    if published == 0 {
        anyhow::bail!("no prompt snapshots were published");
    }
    Ok(())
}

fn knowledge_root_for_scope(
    scope: ContextScope,
    explicit_project_kb: Option<&str>,
) -> Option<PathBuf> {
    match scope {
        ContextScope::Project => explicit_project_kb
            .map(expand_tilde)
            .or_else(|| find_project_root().map(|root| root.join(".prometheus/knowledge"))),
        ContextScope::Shared => Some(global_kb_dir().join("shared")),
        ContextScope::Global => Some(global_kb_dir()),
    }
}

fn context_snippet(description: Option<&str>, content: &str) -> String {
    let source = description
        .filter(|description| !description.trim().is_empty())
        .unwrap_or(content);
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_utf8(&normalized, 360).to_owned()
}

fn rendered_context(output: &ContextOutput) -> String {
    if output.results.is_empty() {
        return String::new();
    }
    let mut rendered = String::from("--- prometheus-knowledge context ---\n");
    for result in &output.results {
        rendered.push_str(&format!(
            "[{}:{}] {}\n{}\n",
            result.scope, result.id, result.title, result.snippet
        ));
    }
    if !output.failures.is_empty() {
        rendered.push_str(&format!(
            "[context-status] failed_scopes={} candidates={}\n",
            output.failures.len(),
            output.candidate_count
        ));
    }
    rendered.push_str("--- end pk context ---\n");
    rendered
}

fn print_context_hook(output: &ContextOutput, max_bytes: usize) {
    let rendered = rendered_context(output);
    print!("{}", truncate_utf8(&rendered, max_bytes));
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn run_codegraph_extract(
    project: Option<PathBuf>,
    output: Option<PathBuf>,
    ci: bool,
) -> anyhow::Result<()> {
    use anyhow::Context;

    // Resolve project root: explicit flag > project detection > cwd
    let project_root = project
        .or_else(find_project_root)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd must exist"));

    let script = project_root.join("scripts").join("codegraph-extract.ts");
    if !script.exists() {
        anyhow::bail!(
            "codegraph-extract.ts not found at {}\n\
             Run `pk codegraph extract` from a project that has scripts/codegraph-extract.ts",
            script.display()
        );
    }

    let mut cmd = std::process::Command::new("npx");
    cmd.arg("tsx").arg(&script);
    if let Some(out) = output {
        cmd.arg("--output").arg(out);
    }
    if ci {
        cmd.arg("--ci");
    }
    cmd.current_dir(&project_root);

    let status = cmd
        .status()
        .with_context(|| format!("failed to run tsx {}", script.display()))?;

    if !status.success() {
        anyhow::bail!("codegraph-extract.ts exited with status {}", status);
    }

    Ok(())
}

/// Resolve the KB directory in priority order:
/// 1. --kb-dir flag / PK_KB_DIR env (explicit override)
/// 2. For shared-scope ingest: ~/.prometheus/knowledge/shared/
/// 3. Project root detected from cwd → <project_root>/.prometheus/knowledge/
/// 4. Global fallback → ~/.prometheus/knowledge/
fn resolve_kb_dir(explicit: Option<&str>, cmd: &Cmd) -> PathBuf {
    if let Some(path) = explicit {
        return expand_tilde(path);
    }

    // Shared scope writes to the global shared subdirectory
    if let Cmd::Ingest {
        scope: KbScope::Shared,
        ..
    } = cmd
    {
        return global_kb_dir().join("shared");
    }

    // Attempt project-root detection
    if let Some(project_root) = find_project_root() {
        let project_kb = project_root.join(".prometheus").join("knowledge");
        return project_kb;
    }

    // Global fallback with info message
    let global = global_kb_dir();
    eprintln!(
        "info: no project root detected; using global KB at {}",
        global.display()
    );
    eprintln!("info: run pk inside a project directory to use per-project KB scoping");
    global
}

/// Walk up from cwd looking for project root markers.
/// Markers (in priority order): .git, .kbd-orchestrator, Cargo.toml, package.json, pyproject.toml
fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let markers = [
        ".git",
        ".kbd-orchestrator",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
    ];

    let mut dir = cwd.as_path();
    loop {
        for marker in &markers {
            if dir.join(marker).exists() {
                return Some(dir.to_path_buf());
            }
        }
        dir = dir.parent()?;
    }
}

fn global_kb_dir() -> PathBuf {
    expand_tilde("~/.prometheus/knowledge")
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Dry-run (default) or execute migration of global KB entries to per-project directories.
async fn run_migrate(execute: bool) -> Result<()> {
    let global = global_kb_dir();

    if !global.exists() {
        println!(
            "Global KB at {} does not exist. Nothing to migrate.",
            global.display()
        );
        return Ok(());
    }

    println!("Migration report — global KB: {}", global.display());
    println!(
        "Mode: {}",
        if execute {
            "EXECUTE"
        } else {
            "DRY-RUN (pass --execute to apply)"
        }
    );
    println!();

    // Read all markdown files in the global KB
    let mut entries = tokio::fs::read_dir(&global).await?;
    let mut count = 0usize;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();

            // Look for a source-project hint in the frontmatter or content
            let project_hint = extract_project_hint(&content);

            match project_hint {
                Some(hint) => {
                    println!(
                        "  [associate] {} → project: {}",
                        path.file_name().unwrap().to_string_lossy(),
                        hint
                    );
                    if execute {
                        // Move to project-scoped directory if the project root exists
                        if let Some(target) = resolve_project_kb_for_hint(&hint) {
                            tokio::fs::create_dir_all(&target).await?;
                            let dest = target.join(path.file_name().unwrap());
                            tokio::fs::rename(&path, &dest).await?;
                            println!("    → moved to {}", dest.display());
                        } else {
                            println!("    → project root not found on disk; left in global KB");
                        }
                    }
                }
                None => {
                    println!(
                        "  [no-hint]   {} → stays in shared KB",
                        path.file_name().unwrap().to_string_lossy()
                    );
                }
            }
            count += 1;
        }
    }

    println!("\nTotal entries scanned: {count}");
    if !execute {
        println!("Run with --execute to apply the migration.");
    }

    Ok(())
}

fn extract_project_hint(content: &str) -> Option<String> {
    // Look for "source_project: <name>" or "project: <name>" in frontmatter
    for line in content.lines().take(20) {
        if let Some(val) = line
            .strip_prefix("source_project:")
            .or_else(|| line.strip_prefix("project:"))
        {
            let hint = val.trim().to_string();
            if !hint.is_empty() {
                return Some(hint);
            }
        }
    }
    None
}

fn resolve_project_kb_for_hint(hint: &str) -> Option<PathBuf> {
    // Search common project parent directories for a matching project name
    let search_parents = [
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("Projects")),
        Some(PathBuf::from("/Users")),
    ];

    for parent in search_parents.iter().flatten() {
        let candidate = parent.join(hint).join(".prometheus").join("knowledge");
        if parent.join(hint).exists() {
            return Some(candidate);
        }
        // Also try nested: Projects/prometheus/<hint>
        let nested = parent
            .join("prometheus")
            .join(hint)
            .join(".prometheus")
            .join("knowledge");
        if parent.join("prometheus").join(hint).exists() {
            return Some(nested);
        }
    }
    None
}

// ── pk doctor ─────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    detail: String,
}

#[derive(Debug, serde::Serialize)]
struct DoctorReport {
    schema_version: u32,
    summary: DoctorSummary,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug, serde::Serialize)]
struct DoctorSummary {
    passed: usize,
    warned: usize,
    failed: usize,
}

fn run_doctor(json_output: bool, project_kb: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let plugin_root = std::env::var_os("PROMETHEUS_PLUGIN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".prometheus/plugins/prometheus-skill-pack"));
    let hook_log = home.join(".prometheus/hooks.log");
    let hook_status = fs::metadata(&hook_log)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
        .unwrap_or(false);
    let active_generation = active_plugin_generation(&plugin_root);
    let stable_scripts = [
        "karpathy-hook-dispatch.sh",
        "detect-project-context.sh",
        "memory-outbox-flush.sh",
        "pk-health.sh",
    ];
    let stable_healthy = active_generation.is_some()
        && stable_scripts.iter().all(|script| {
            plugin_root
                .join("stable")
                .join(script)
                .canonicalize()
                .ok()
                .is_some_and(|path| path.starts_with(plugin_root.join("generations")))
        });

    let global_kb = home.join(".prometheus/knowledge");
    let snapshot_results = [
        ("project", project_kb.to_path_buf()),
        ("shared", global_kb.join("shared")),
        ("global", global_kb),
    ]
    .into_iter()
    .map(|(scope, root)| {
        read_prompt_snapshot(&root, scope)
            .map(|snapshot| format!("{scope}={}", snapshot.generation))
            .map_err(|error| format!("{scope}: {error}"))
    })
    .collect::<Vec<_>>();
    let snapshots_healthy = snapshot_results.iter().all(|result| result.is_ok());
    let snapshot_detail = snapshot_results
        .iter()
        .map(|result| match result {
            Ok(value) | Err(value) => value.as_str(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    let queue = home.join(".prometheus/learning-queue");
    let unsettled = [
        "pending",
        "processing",
        "retry",
        "dead-letter",
        "memory/pending",
        "memory/submitting",
        "memory/accepted",
        "memory/retry",
        "memory/dead-letter",
    ]
    .into_iter()
    .map(|relative| count_json(&queue.join(relative)))
    .sum::<usize>();
    let configured_worker = std::env::var_os("PROMETHEUS_LEARNING_WORKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/bin/prometheus-learning-worker"));
    let worker_installed = configured_worker.is_file()
        || Path::new("/usr/local/bin/prometheus-learning-worker").is_file();
    let queue_healthy = worker_installed && unsettled == 0;

    let checks = vec![
        DoctorCheck {
            name: "hooks-log-path",
            status: if hook_status { "PASS" } else { "FAIL" },
            detail: format!("{} (required mode 0600)", hook_log.display()),
        },
        DoctorCheck {
            name: "plugin-generation",
            status: if active_generation.is_some() {
                "PASS"
            } else {
                "FAIL"
            },
            detail: active_generation.unwrap_or_else(|| {
                format!("no valid 14-target generation at {}", plugin_root.display())
            }),
        },
        DoctorCheck {
            name: "stable-dispatchers",
            status: if stable_healthy { "PASS" } else { "FAIL" },
            detail: format!("4 stable dispatchers under {}", plugin_root.display()),
        },
        DoctorCheck {
            name: "prompt-snapshots",
            status: if snapshots_healthy { "PASS" } else { "FAIL" },
            detail: snapshot_detail,
        },
        DoctorCheck {
            name: "learning-queue",
            status: if queue_healthy { "PASS" } else { "FAIL" },
            detail: format!(
                "worker {}, unsettled records {unsettled}, queue {}",
                if worker_installed {
                    "installed"
                } else {
                    "missing"
                },
                queue.display()
            ),
        },
        DoctorCheck {
            name: "kb-scoping",
            status: if project_kb.is_dir() { "PASS" } else { "FAIL" },
            detail: format!("project knowledge root {}", project_kb.display()),
        },
    ];
    let report = DoctorReport {
        schema_version: 2,
        summary: DoctorSummary {
            passed: checks.iter().filter(|check| check.status == "PASS").count(),
            warned: checks.iter().filter(|check| check.status == "WARN").count(),
            failed: checks.iter().filter(|check| check.status == "FAIL").count(),
        },
        checks,
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("\npk doctor — deterministic learning health\n");
        for check in &report.checks {
            let icon = if check.status == "PASS" { "✓" } else { "✗" };
            println!(
                "  {icon} [{:<24}] {} — {}",
                check.name, check.status, check.detail
            );
        }
        println!(
            "\n  {} passed, {} warned, {} failed\n",
            report.summary.passed, report.summary.warned, report.summary.failed
        );
    }
    if report.summary.failed > 0 {
        anyhow::bail!("pk doctor detected failing checks");
    }
    Ok(())
}

fn active_plugin_generation(plugin_root: &Path) -> Option<String> {
    let current = fs::read_link(plugin_root.join("current")).ok()?;
    let resolved = plugin_root.join(current);
    let canonical = resolved.canonicalize().ok()?;
    if !canonical.starts_with(plugin_root.join("generations")) {
        return None;
    }
    let generation = canonical.file_name()?.to_str()?.to_owned();
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(canonical.join("manifest.json")).ok()?).ok()?;
    (manifest["generation"] == generation
        && manifest["targetPayloads"].as_array().map(Vec::len) == Some(14))
    .then_some(generation)
}

fn count_json(path: &Path) -> usize {
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .count()
}

// ── pk init ───────────────────────────────────────────────────────────────────

fn run_init(name: Option<String>, stack: Option<String>, yes: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Resolve project name
    let project_name = name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-project")
            .to_owned()
    });

    let stack_name = stack.unwrap_or_else(|| detect_stack(&cwd));

    println!("\npk init — Prometheus project onboarding\n");
    println!("  Project name: {project_name}");
    println!("  Stack:        {stack_name}");
    println!("  Directory:    {}", cwd.display());
    println!();

    if !yes {
        print!("Continue? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Step 1: Create per-project KB directory
    let prometheus_dir = cwd.join(".prometheus");
    let kb_dir = prometheus_dir.join("knowledge");
    std::fs::create_dir_all(&kb_dir)?;
    println!("✓ Created KB directory: {}", kb_dir.display());

    // Step 2: Create .prometheus/hooks/ symlink target if missing
    let hooks_dir = prometheus_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    // Step 3: Write a starter CLAUDE.md (non-destructive — skip if already exists)
    let claude_md = cwd.join("CLAUDE.md");
    if !claude_md.exists() {
        let content = generate_claude_md(&project_name, &stack_name);
        std::fs::write(&claude_md, content)?;
        println!("✓ Generated CLAUDE.md");
    } else {
        println!("· CLAUDE.md already exists — skipping");
    }

    // Step 4: Write .gitignore entry for .prometheus/ artifacts (non-destructive)
    let gitignore = cwd.join(".gitignore");
    let prometheus_entry = "\n# Prometheus runtime artifacts\n.prometheus/knowledge/\n.prometheus/hooks.log\nSCRATCHPAD.md\n";
    if gitignore.exists() {
        let existing = std::fs::read_to_string(&gitignore)?;
        if !existing.contains(".prometheus/knowledge") {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore)
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(prometheus_entry.as_bytes())
                })?;
            println!("✓ Added .prometheus/knowledge/ to .gitignore");
        } else {
            println!("· .gitignore already has .prometheus/ entry");
        }
    } else {
        std::fs::write(&gitignore, prometheus_entry.trim_start())?;
        println!("✓ Created .gitignore with .prometheus/ entry");
    }

    println!("\nDone. Next steps:");
    println!("  1. Run `pk ingest <file>` to add your first document to the KB");
    println!("  2. Run `pk doctor` to verify your Prometheus setup");
    println!("  3. Run `/kbd-assess` to start the KBD lifecycle for this project");
    println!();

    Ok(())
}

fn detect_stack(dir: &std::path::Path) -> String {
    if dir.join("Cargo.toml").exists() {
        return "rust".into();
    }
    if dir.join("package.json").exists() {
        return "typescript".into();
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        return "python".into();
    }
    if dir.join("go.mod").exists() {
        return "go".into();
    }
    "unknown".into()
}

fn generate_claude_md(project_name: &str, stack: &str) -> String {
    format!(
        r#"# CLAUDE.md

This file provides guidance to Claude Code when working in this repository.

## Project

**Name**: {project_name}
**Stack**: {stack}

## Memory

Check `~/.claude/projects/.../memory/MEMORY.md` at the start of each session.

## Progress Signaling (MANDATORY)

Emit before and after every phase and task:

```
Starting phase N out of M: <name>
Starting task N out of M: <name>
Completed task N out of M: <name>
Completed phase N out of M: <name>
```

## KB Scoping

This project uses a per-project knowledge base at `.prometheus/knowledge/`.
Run `pk ingest <file>` to add documents. Run `pk focus <topic>` to retrieve context.

## KBD Lifecycle

When implementing features:
1. `/kbd-assess` — gap analysis
2. `/kbd-plan` — ordered change list
3. `/kbd-execute` — implement changes
4. `/kbd-reflect` — summarize and advance

## References

- [Prometheus Skill Pack](https://github.com/gqadonis/prometheus-skill-pack)
"#
    )
}
