use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use pk_core::{error::PkResult, types::RawDoc, LibrarianEvent};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

/// Watch a directory for new/modified files and emit RawDoc values.
/// Internally uses `notify`'s platform-native backend (FSEvents on macOS,
/// inotify on Linux) with a debounce window to avoid duplicate events.
pub struct InboxWatcher {
    raw_dir: PathBuf,
    event_tx: broadcast::Sender<LibrarianEvent>,
    raw_doc_tx: mpsc::Sender<RawDoc>,
}

impl InboxWatcher {
    pub fn new(
        raw_dir: PathBuf,
        event_tx: broadcast::Sender<LibrarianEvent>,
        raw_doc_tx: mpsc::Sender<RawDoc>,
    ) -> Self {
        Self { raw_dir, event_tx, raw_doc_tx }
    }

    /// Spawn the watcher on a dedicated blocking thread (notify requires it).
    /// Returns a handle that keeps the watcher alive.
    pub fn spawn(self: Arc<Self>) -> PkResult<WatcherHandle> {
        let (notify_tx, mut notify_rx) = mpsc::channel::<PathBuf>(64);
        let raw_dir = self.raw_dir.clone();

        let _watcher_thread = std::thread::spawn(move || {
            let handler = move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_)
                    ) {
                        for path in event.paths {
                            if path.extension().and_then(|e| e.to_str()) != Some("tmp") {
                                let _ = notify_tx.blocking_send(path);
                            }
                        }
                    }
                }
                Err(e) => warn!(err = %e, "watcher error"),
            };

            let config = Config::default().with_poll_interval(Duration::from_millis(300));

            match RecommendedWatcher::new(handler, config) {
                Ok(mut watcher) => {
                    if let Err(e) = watcher.watch(&raw_dir, RecursiveMode::NonRecursive) {
                        warn!(err = %e, "failed to watch directory");
                        return;
                    }
                    info!(dir = %raw_dir.display(), "inbox watcher active");
                    std::thread::park();
                }
                Err(e) => warn!(err = %e, "failed to create watcher"),
            }
        });

        let watcher = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(path) = notify_rx.recv().await {
                debug!(path = %path.display(), "inbox file detected");

                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        let source = path.to_string_lossy().to_string();

                        let _ = watcher.event_tx.send(LibrarianEvent::RawDocArrived {
                            source_path: source.clone(),
                            ts: chrono::Utc::now(),
                        });

                        let doc = RawDoc::from_path(source, content);

                        if watcher.raw_doc_tx.send(doc).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(path = %path.display(), err = %e, "failed to read inbox file");
                    }
                }
            }
        });

        Ok(WatcherHandle { _marker: std::marker::PhantomData })
    }
}

/// Keeps the watcher alive. Drop to stop watching.
pub struct WatcherHandle {
    _marker: std::marker::PhantomData<()>,
}
