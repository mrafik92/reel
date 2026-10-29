//! API: axum routes, OpenAPI, WebSocket; backs the `reel-server` binary.

pub mod dto;
pub mod enrich;
pub mod error;
mod routes;

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use reel_core::ServerEvent;
use reel_metadata::ImageCache;
use reel_scanner::{LibraryWatcher, MetadataProvider, Scanner};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<reel_core::Config>,
    pub events: broadcast::Sender<ServerEvent>,
    pub scanner: Scanner,
    pub images: ImageCache,
    watcher: Arc<tokio::sync::Mutex<Option<LibraryWatcher>>>,
}

impl AppState {
    pub fn new(db: SqlitePool, config: reel_core::Config) -> Self {
        let (events, _) = broadcast::channel(256);
        let images = ImageCache::new(&config.data_dir);
        Self {
            db,
            scanner: Scanner::new().with_events(events.clone()),
            images,
            config: Arc::new(config),
            events,
            watcher: Arc::default(),
        }
    }

    /// Use `m` as the scanner's metadata hook.
    pub fn with_metadata(mut self, m: Arc<dyn MetadataProvider>) -> Self {
        self.scanner = self.scanner.with_metadata(m);
        self
    }

    /// Scan a library on a background task (scans are serialized by the scanner).
    pub fn spawn_scan(&self, library_id: i64) {
        let (scanner, db) = (self.scanner.clone(), self.db.clone());
        tokio::spawn(async move {
            match scanner.scan_library(&db, library_id).await {
                Ok(s) => tracing::info!(library_id, ?s, "scan finished"),
                Err(e) => tracing::warn!(library_id, "scan failed: {e}"),
            }
        });
    }

    /// (Re)start the filesystem watcher over all library paths.
    pub async fn restart_watcher(&self) {
        let mut guard = self.watcher.lock().await;
        *guard = None;
        match LibraryWatcher::start(
            self.scanner.clone(),
            self.db.clone(),
            Duration::from_secs(2),
        )
        .await
        {
            Ok(w) => *guard = Some(w),
            Err(e) => tracing::warn!("cannot start library watcher: {e}"),
        }
    }

    /// Validate + insert a library, kick off a scan, restart the watcher.
    /// Errors are user-facing validation messages.
    pub async fn create_library(
        &self,
        name: &str,
        kind: dto::LibraryKind,
        paths: &[impl AsRef<std::path::Path>],
    ) -> Result<i64, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name must not be empty".into());
        }
        if paths.is_empty() {
            return Err("at least one path is required".into());
        }
        let mut resolved: Vec<String> = Vec::new();
        for p in paths {
            let p = p.as_ref();
            let c = std::fs::canonicalize(p)
                .ok()
                .filter(|c| c.is_dir())
                .ok_or_else(|| format!("path is not a directory: {}", p.display()))?;
            resolved.push(c.to_string_lossy().into_owned());
        }
        let json = serde_json::to_string(&resolved).map_err(|e| e.to_string())?;
        let id = sqlx::query("INSERT INTO libraries (name, kind, paths_json) VALUES (?, ?, ?)")
            .bind(name)
            .bind(kind.as_str())
            .bind(json)
            .execute(&self.db)
            .await
            .map_err(|e| e.to_string())?
            .last_insert_rowid();
        self.spawn_scan(id);
        self.restart_watcher().await;
        Ok(id)
    }
}

/// Create `[[libraries]]` from config that don't exist yet (by name).
pub async fn bootstrap_libraries(state: &AppState) {
    for l in &state.config.libraries {
        let exists: Result<Option<i64>, _> =
            sqlx::query_scalar("SELECT id FROM libraries WHERE name = ?")
                .bind(&l.name)
                .fetch_optional(&state.db)
                .await;
        if !matches!(exists, Ok(None)) {
            continue;
        }
        let Some(kind) = dto::LibraryKind::parse(&l.kind) else {
            tracing::warn!(library = %l.name, "invalid kind '{}'", l.kind);
            continue;
        };
        if let Err(e) = state.create_library(&l.name, kind, &l.paths).await {
            tracing::warn!(library = %l.name, "bootstrap failed: {e}");
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Reel API", version = env!("CARGO_PKG_VERSION")),
    components(schemas(ServerEvent, reel_core::RequestState, error::ErrorBody))
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
        .routes(routes!(routes::list_libraries, routes::create_library))
        .routes(routes!(routes::scan_library))
        .routes(routes!(routes::library_items))
        .routes(routes!(routes::get_item))
        .routes(routes!(routes::item_children))
        .routes(routes!(routes::home))
        .routes(routes!(routes::search))
        .routes(routes!(routes::image))
        .routes(routes!(routes::direct))
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
    let router = match state.config.web_dir.clone() {
        Some(dir) => {
            let index: PathBuf = dir.join("index.html");
            router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => router,
    };
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
