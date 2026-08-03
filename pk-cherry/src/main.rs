// jemalloc: tuned for a long-running SSE server with burst ingestion traffic
#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: Jemalloc = Jemalloc;

use anyhow::Result;
use clap::Parser;
use pk_librarian::{Librarian, ModelRouter};
use pk_mcp::{McpServer, ReadinessHandle};
use pk_store::MarkdownStore;
use pk_watcher::{spawn_wiki_watcher, InboxWatcher, WikiWatchEvent};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{broadcast, mpsc};
use tracing::info;

/// pk-cherry: Prometheus Knowledge MCP bridge for Cherry Studio
///
/// Starts a local MCP server that Cherry Studio can connect to.
/// Drop files into <kb-dir>/raw/ and they will be auto-compiled.
///
/// Cherry Studio MCP config:
///   { "name": "prometheus-knowledge", "url": "http://localhost:8942/mcp", "transport": "sse" }
#[derive(Debug, Parser)]
#[command(name = "pk-cherry", version, about)]
struct Args {
    #[arg(long, env = "PK_KB_DIR", default_value = "~/.prometheus/knowledge")]
    kb_dir: String,

    #[arg(long, env = "PK_BIND", default_value = "127.0.0.1:8942")]
    bind: String,

    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(&args.log_level)
        .json()
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "pk-cherry starting");

    let kb_dir = expand_tilde(&args.kb_dir);
    info!(kb_dir = %kb_dir.display(), "knowledge base directory");

    let store = Arc::new(MarkdownStore::open(&kb_dir).await?);
    info!(entries = store.entry_count().await, "store opened");
    let readiness = ReadinessHandle::new(&kb_dir);
    readiness
        .update_store(&store.readiness_report().await)
        .await;

    let (event_tx, _) = broadcast::channel::<pk_core::LibrarianEvent>(256);
    let (raw_tx, raw_rx) = mpsc::channel(32);

    let router = ModelRouter::from_env();
    let librarian = Arc::new(Librarian::new(Arc::clone(&store), router, event_tx.clone()));

    let watcher = Arc::new(InboxWatcher::new(
        store.raw_dir().to_path_buf(),
        event_tx.clone(),
        raw_tx,
    ));
    let _watch_handle = watcher.spawn()?;
    info!("inbox watcher spawned");

    let (wiki_watch_tx, mut wiki_watch_rx) = mpsc::channel(64);
    let _wiki_watch_handle = spawn_wiki_watcher(store.wiki_dir().to_path_buf(), wiki_watch_tx)?;
    {
        let store = Arc::clone(&store);
        let readiness = readiness.clone();
        tokio::spawn(async move {
            while let Some(event) = wiki_watch_rx.recv().await {
                match event {
                    WikiWatchEvent::Started => readiness.set_watcher("active").await,
                    WikiWatchEvent::Failed(error) => {
                        readiness.set_watcher(format!("failed: {error}")).await;
                    }
                    WikiWatchEvent::Changed => {
                        // Debounce bursts from atomic rename/create sequences.
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        while matches!(wiki_watch_rx.try_recv(), Ok(WikiWatchEvent::Changed)) {}
                        match store.reconcile_from_disk().await {
                            Ok(report) => readiness.update_store(&report).await,
                            Err(error) => {
                                readiness
                                    .set_watcher(format!("reconcile_failed: {error}"))
                                    .await;
                            }
                        }
                    }
                }
            }
            readiness.set_watcher("stopped").await;
        });
    }

    {
        let lib = Arc::clone(&librarian);
        tokio::spawn(async move {
            lib.run_inbox_loop(raw_rx).await;
        });
    }

    let server =
        McpServer::new_with_readiness(Arc::clone(&librarian), event_tx, readiness, &args.bind);

    info!(bind = %args.bind, "MCP server starting");
    println!(
        "\n✓ pk-cherry running — connect Cherry Studio:\n  {{ \"name\": \"prometheus-knowledge\", \"url\": \"http://{}/mcp\", \"transport\": \"sse\" }}\n",
        args.bind
    );

    server.serve().await?;
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
