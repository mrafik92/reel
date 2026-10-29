//! API: axum routes, OpenAPI, WebSocket; backs the `reel-server` binary.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use reel_core::ServerEvent;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<reel_core::Config>,
    pub events: broadcast::Sender<ServerEvent>,
}

impl AppState {
    pub fn new(db: SqlitePool, config: reel_core::Config) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            db,
            config: Arc::new(config),
            events,
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Reel API", version = env!("CARGO_PKG_VERSION")),
    components(schemas(ServerEvent, reel_core::RequestState))
)]
struct ApiDoc;

/// Health response.
#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: String,
    pub version: String,
}

/// Liveness check.
#[utoipa::path(get, path = "/health", responses((status = 200, body = Health)))]
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// WebSocket stream of [`ServerEvent`]s as JSON text frames.
#[utoipa::path(
    get,
    path = "/events",
    responses((status = 101, description = "WebSocket upgrade; frames are JSON ServerEvent"))
)]
pub async fn events(ws: WebSocketUpgrade, State(st): State<AppState>) -> impl IntoResponse {
    // TODO(auth): authenticate via `?token=` query param before upgrading.
    let rx = st.events.subscribe();
    ws.on_upgrade(move |socket| forward_events(socket, rx))
}

async fn forward_events(mut socket: WebSocket, mut rx: broadcast::Receiver<ServerEvent>) {
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}

fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(events))
}

/// The OpenAPI document (paths prefixed with `/api/v1`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_, api) = OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", api_router())
        .split_for_parts();
    api
}

/// Build the full application router.
pub fn app(state: AppState) -> Router {
    let (router, api) = OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", api_router())
        .split_for_parts();
    let spec = Arc::new(api);
    router
        .route(
            "/api/v1/openapi.json",
            axum::routing::get(move || {
                let spec = spec.clone();
                async move { Json(spec) }
            }),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_ok() {
        let db = reel_core::db::connect_url("sqlite::memory:").await.unwrap();
        let app = app(AppState::new(db, reel_core::Config::default()));
        let res = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert!(v["version"].is_string());
    }
}
