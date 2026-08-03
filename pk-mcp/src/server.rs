use crate::tools::{dispatch, McpRequest};
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use pk_core::LibrarianEvent;
use pk_librarian::Librarian;
use pk_store::StoreReconcileReport;
use serde::Serialize;
use std::{convert::Infallible, path::Path, sync::Arc, time::Duration};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub librarian: Arc<Librarian>,
    pub event_tx: broadcast::Sender<LibrarianEvent>,
    pub readiness: ReadinessHandle,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadinessSnapshot {
    pub status: &'static str,
    pub store_path: String,
    pub indexed_count: usize,
    pub on_disk_count: usize,
    pub parse_failures: usize,
    pub last_reload: Option<chrono::DateTime<chrono::Utc>>,
    pub watcher_status: String,
    pub index_fresh: bool,
}

#[derive(Clone)]
pub struct ReadinessHandle(Arc<RwLock<ReadinessSnapshot>>);

impl ReadinessHandle {
    pub fn new(store_path: impl AsRef<Path>) -> Self {
        Self(Arc::new(RwLock::new(ReadinessSnapshot {
            status: "not_ready",
            store_path: store_path.as_ref().display().to_string(),
            indexed_count: 0,
            on_disk_count: 0,
            parse_failures: 0,
            last_reload: None,
            watcher_status: "initializing".to_string(),
            index_fresh: false,
        })))
    }

    pub async fn update_store(&self, report: &StoreReconcileReport) {
        let mut state = self.0.write().await;
        state.indexed_count = report.indexed_count;
        state.on_disk_count = report.on_disk_count;
        state.parse_failures = report.parse_failures;
        state.last_reload = Some(report.last_reload);
        state.index_fresh = report.indexed_count + report.parse_failures == report.on_disk_count;
        state.status = if state.index_fresh && state.watcher_status == "active" {
            "ready"
        } else {
            "not_ready"
        };
    }

    pub async fn set_watcher(&self, status: impl Into<String>) {
        let mut state = self.0.write().await;
        state.watcher_status = status.into();
        state.status = if state.index_fresh && state.watcher_status == "active" {
            "ready"
        } else {
            "not_ready"
        };
    }

    pub async fn snapshot(&self) -> ReadinessSnapshot {
        self.0.read().await.clone()
    }
}

pub struct McpServer {
    state: AppState,
    bind_addr: String,
}

impl McpServer {
    pub fn new(
        librarian: Arc<Librarian>,
        event_tx: broadcast::Sender<LibrarianEvent>,
        bind_addr: impl Into<String>,
    ) -> Self {
        let store_path = librarian
            .store
            .wiki_dir()
            .parent()
            .unwrap_or_else(|| librarian.store.wiki_dir());
        let readiness = ReadinessHandle::new(store_path);
        Self::new_with_readiness(librarian, event_tx, readiness, bind_addr)
    }

    pub fn new_with_readiness(
        librarian: Arc<Librarian>,
        event_tx: broadcast::Sender<LibrarianEvent>,
        readiness: ReadinessHandle,
        bind_addr: impl Into<String>,
    ) -> Self {
        Self {
            state: AppState {
                librarian,
                event_tx,
                readiness,
            },
            bind_addr: bind_addr.into(),
        }
    }

    pub fn router(state: AppState) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/mcp", post(mcp_handler))
            .route("/events", get(sse_handler))
            .route("/health", get(health_handler))
            .route("/ready", get(ready_handler))
            .layer(cors)
            .with_state(state)
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let addr: std::net::SocketAddr = self.bind_addr.parse()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(addr = %addr, "pk-mcp server listening");
        axum::serve(listener, Self::router(self.state)).await?;
        Ok(())
    }
}

async fn mcp_handler(
    State(state): State<AppState>,
    Json(req): Json<McpRequest>,
) -> axum::response::Response {
    // JSON-RPC notifications (no `id`, e.g. notifications/initialized) expect no
    // response body — acknowledge with 202 so the client proceeds.
    if req.id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }
    Json(dispatch(&state.librarian, req).await).into_response()
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(event) => {
            let json = serde_json::to_string(&event).unwrap_or_default();
            let event_type = match &event {
                LibrarianEvent::Compiled { .. } => "compiled",
                LibrarianEvent::LintCompleted { .. } => "lint_completed",
                LibrarianEvent::Focused { .. } => "focused",
                LibrarianEvent::Updated { .. } => "updated",
                LibrarianEvent::RawDocArrived { .. } => "raw_doc_arrived",
                LibrarianEvent::Error { .. } => "error",
            };
            Some(Ok(Event::default().event(event_type).data(json)))
        }
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.readiness.snapshot().await;
    let status = if snapshot.status == "ready" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pk_librarian::ModelRouter;
    use pk_store::MarkdownStore;

    async fn fixture_state() -> (AppState, tempfile::TempDir) {
        let fixture = tempfile::tempdir().unwrap();
        let store = Arc::new(MarkdownStore::open(fixture.path()).await.unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let librarian = Arc::new(Librarian::new(
            Arc::clone(&store),
            ModelRouter::from_env(),
            event_tx.clone(),
        ));
        let readiness = ReadinessHandle::new(fixture.path());
        readiness
            .update_store(&store.readiness_report().await)
            .await;
        (
            AppState {
                librarian,
                event_tx,
                readiness,
            },
            fixture,
        )
    }

    #[tokio::test]
    async fn health_is_static_even_when_readiness_is_unavailable() {
        let (state, _fixture) = fixture_state().await;
        state.readiness.set_watcher("failed: fixture").await;
        assert_eq!(
            health_handler().await.into_response().status(),
            StatusCode::OK
        );
        assert_eq!(
            ready_handler(State(state)).await.into_response().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn ready_requires_a_fresh_index_and_active_watcher() {
        let (state, _fixture) = fixture_state().await;
        state.readiness.set_watcher("active").await;
        let snapshot = state.readiness.snapshot().await;
        assert_eq!(snapshot.indexed_count, snapshot.on_disk_count);
        assert!(snapshot.index_fresh);
        assert_eq!(
            ready_handler(State(state)).await.into_response().status(),
            StatusCode::OK
        );
    }
}
