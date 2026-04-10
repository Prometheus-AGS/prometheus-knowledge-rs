use crate::tools::{dispatch, McpRequest, McpResponse};
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
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub librarian: Arc<Librarian>,
    pub event_tx: broadcast::Sender<LibrarianEvent>,
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
        Self { state: AppState { librarian, event_tx }, bind_addr: bind_addr.into() }
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
) -> Json<McpResponse> {
    Json(dispatch(&state.librarian, req).await)
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(event) => {
            let json = serde_json::to_string(&event).unwrap_or_default();
            let event_type = match &event {
                LibrarianEvent::Compiled { .. }      => "compiled",
                LibrarianEvent::LintCompleted { .. } => "lint_completed",
                LibrarianEvent::Focused { .. }       => "focused",
                LibrarianEvent::Updated { .. }       => "updated",
                LibrarianEvent::RawDocArrived { .. } => "raw_doc_arrived",
                LibrarianEvent::Error { .. }         => "error",
            };
            Some(Ok(Event::default().event(event_type).data(json)))
        }
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let count = state.librarian.store.entry_count().await;
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "entry_count": count })))
}
