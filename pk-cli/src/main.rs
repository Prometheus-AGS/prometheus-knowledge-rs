#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: Jemalloc = Jemalloc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use pk_core::types::RawDoc;
use pk_librarian::{Librarian, ModelRouter};
use pk_store::MarkdownStore;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::broadcast;

/// KB scope: project-local (default) or globally shared across projects.
#[derive(Debug, Clone, ValueEnum)]
enum KbScope {
    /// Write to <project_root>/.prometheus/knowledge/ (default)
    Project,
    /// Write to ~/.prometheus/knowledge/shared/ (cross-project patterns)
    Shared,
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
    },
    /// Build a focused mini-KB for a topic and print to stdout
    Focus {
        #[arg()]
        topic: String,
        #[arg(long, default_value_t = 10)]
        k: usize,
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
    /// Migrate global KB entries to per-project directories (dry-run by default)
    MigrateToPerProject {
        /// Execute the migration (default is dry-run; review output first)
        #[arg(long, default_value_t = false)]
        execute: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    // Resolve KB directory: explicit flag > env > project-root > global fallback
    let kb_dir = resolve_kb_dir(cli.kb_dir.as_deref(), &cli.command);

    // For migrate command we don't open the store at the resolved path yet
    if let Cmd::MigrateToPerProject { execute } = &cli.command {
        return run_migrate(*execute).await;
    }

    let store = Arc::new(MarkdownStore::open(&kb_dir).await?);
    let (event_tx, _) = broadcast::channel(64);
    let librarian = Arc::new(Librarian::new(Arc::clone(&store), ModelRouter::from_env(), event_tx));

    match cli.command {
        Cmd::Ingest { file, source, scope, yes } => {
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
            println!("✓ compiled → {} [{}]", entry.title, entry.id);
        }

        Cmd::Lint { fix } => {
            let reports = librarian.lint().await?;
            if reports.is_empty() {
                println!("✓ no issues found");
                return Ok(());
            }
            let mut fixed = 0usize;
            for report in &reports {
                let icon = match report.severity {
                    pk_core::types::LintSeverity::Error   => "✗",
                    pk_core::types::LintSeverity::Warning => "⚠",
                    pk_core::types::LintSeverity::Info    => "ℹ",
                };
                let entry_label = report.entry_id.as_ref()
                    .map(|id| id.as_str().to_owned())
                    .unwrap_or_else(|| "(global)".into());
                println!("{icon} [{entry_label}] {} — {}", report.severity, report.issue);
                println!("  → {}", report.suggestion);
                if fix && report.auto_fixable {
                    match librarian.auto_fix(report).await {
                        Ok(entry) => { println!("  ✓ fixed → revision {}", entry.revision); fixed += 1; }
                        Err(e)    => println!("  ✗ fix failed: {e}"),
                    }
                }
            }
            println!("\n{} issue(s)", reports.len());
            if fix { println!("{fixed} auto-fixed"); }
        }

        Cmd::Focus { topic, k } => {
            println!("{}", librarian.focus(&topic, k).await?);
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
            println!("# {}\n\ntags: {}\n\n{}", entry.title, entry.tags.join(", "), entry.content);
        }

        Cmd::List => {
            let entries = store.snapshot().await?;
            if entries.is_empty() {
                println!("(empty knowledge base)");
            } else {
                for e in &entries { println!("[{}] {} (rev {})", e.id, e.title, e.revision); }
                println!("\n{} entries", entries.len());
            }
        }

        Cmd::Stats => {
            let entries = store.snapshot().await?;
            let total_tags: usize  = entries.iter().map(|e| e.tags.len()).sum();
            let total_links: usize = entries.iter().map(|e| e.links.len()).sum();
            println!("entries:     {}", entries.len());
            println!("total tags:  {total_tags}");
            println!("total links: {total_links}");
            println!("kb dir:      {}", kb_dir.display());
        }

        Cmd::MigrateToPerProject { .. } => unreachable!("handled above"),
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
    if let Cmd::Ingest { scope: KbScope::Shared, .. } = cmd {
        return global_kb_dir().join("shared");
    }

    // Attempt project-root detection
    if let Some(project_root) = find_project_root() {
        let project_kb = project_root.join(".prometheus").join("knowledge");
        return project_kb;
    }

    // Global fallback with info message
    let global = global_kb_dir();
    eprintln!("info: no project root detected; using global KB at {}", global.display());
    eprintln!("info: run pk inside a project directory to use per-project KB scoping");
    global
}

/// Walk up from cwd looking for project root markers.
/// Markers (in priority order): .git, .kbd-orchestrator, Cargo.toml, package.json, pyproject.toml
fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let markers = [".git", ".kbd-orchestrator", "Cargo.toml", "package.json", "pyproject.toml"];

    let mut dir = cwd.as_path();
    loop {
        for marker in &markers {
            if dir.join(marker).exists() {
                return Some(dir.to_path_buf());
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
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
        println!("Global KB at {} does not exist. Nothing to migrate.", global.display());
        return Ok(());
    }

    println!("Migration report — global KB: {}", global.display());
    println!("Mode: {}", if execute { "EXECUTE" } else { "DRY-RUN (pass --execute to apply)" });
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
                    println!("  [associate] {} → project: {}", path.file_name().unwrap().to_string_lossy(), hint);
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
                    println!("  [no-hint]   {} → stays in shared KB", path.file_name().unwrap().to_string_lossy());
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
        if let Some(val) = line.strip_prefix("source_project:").or_else(|| line.strip_prefix("project:")) {
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
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Projects")),
        Some(PathBuf::from("/Users")),
    ];

    for parent_opt in &search_parents {
        if let Some(parent) = parent_opt {
            let candidate = parent.join(hint).join(".prometheus").join("knowledge");
            if parent.join(hint).exists() {
                return Some(candidate);
            }
            // Also try nested: Projects/prometheus/<hint>
            let nested = parent.join("prometheus").join(hint).join(".prometheus").join("knowledge");
            if parent.join("prometheus").join(hint).exists() {
                return Some(nested);
            }
        }
    }
    None
}
