#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: Jemalloc = Jemalloc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use pk_core::types::RawDoc;
use pk_librarian::{Librarian, ModelRouter};
use pk_store::MarkdownStore;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::broadcast;

#[derive(Debug, Parser)]
#[command(name = "pk", version, about = "Prometheus Knowledge CLI")]
struct Cli {
    #[arg(long, env = "PK_KB_DIR", default_value = "~/.prometheus/knowledge", global = true)]
    kb_dir: String,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let kb_dir = expand_tilde(&cli.kb_dir);
    let store = Arc::new(MarkdownStore::open(&kb_dir).await?);
    let (event_tx, _) = broadcast::channel(64);
    let librarian = Arc::new(Librarian::new(Arc::clone(&store), ModelRouter::from_env(), event_tx));

    match cli.command {
        Cmd::Ingest { file, source } => {
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
    }

    Ok(())
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}
