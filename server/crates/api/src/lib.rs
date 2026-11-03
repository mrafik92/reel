//! API: axum routes, OpenAPI, WebSocket; backs the `reel-server` binary.

pub mod auth;
mod browse;
pub mod cast;
pub mod dto;
pub mod enrich;
pub mod error;
mod extras;
mod iptv;
pub mod netutil;

/// Test hook for the profile sanitizer.
#[doc(hidden)]
pub fn sanitize_profile_for_tests(
    u: &reel_auth::User,
    p: reel_playback::DeviceProfile,
) -> reel_playback::DeviceProfile {
    prefs::sanitize_profile(u, p)
}
mod playback;
pub mod prefs;
mod requests;
mod routes;
mod stream;
pub mod vod;
pub mod ws;

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Method, header},
    response::Response,
};
use reel_auth::{AuthOptions, AuthService, User};
use reel_core::ServerEvent;
use reel_metadata::ImageCache;
use reel_playback::{HwAccelStatus, SessionConfig, SessionManager};
use reel_requests::RequestsService;
use reel_scanner::{LibraryWatcher, MetadataProvider, Scanner};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
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
    pub playback: Arc<SessionManager>,
    pub hwaccel: Arc<HwAccelStatus>,
    pub requests: RequestsService,
    pub auth: AuthService,
    pub iptv: Arc<reel_iptv::IptvService>,
    /// Running VOD playbacks (provider URLs stay server-side).
    pub vod: Arc<vod::VodRuntime>,
    /// DLNA renderers and running casts.
    pub cast: Arc<cast::CastRuntime>,
    /// WebSocket tickets and per-user socket slots.
    pub ws: Arc<ws::WsHub>,
    /// TMDB client (set by [`AppState::with_tmdb`]).
    pub tmdb: Option<reel_metadata::TmdbClient>,
    pub omdb: Option<reel_requests::reviews::OmdbClient>,
    pub trakt: Option<reel_requests::reviews::TraktClient>,
    watcher: Arc<tokio::sync::Mutex<Option<LibraryWatcher>>>,
}

impl AppState {
    pub fn new(db: SqlitePool, config: reel_core::Config) -> Self {
        let (events, _) = broadcast::channel(256);
        let images = ImageCache::new(&config.data_dir);
        let hw = HwAccelStatus::software();
        let playback = new_playback(&config, hw.selected.clone());
        let requests = RequestsService::new(db.clone(), None, events.clone());
        let key = match config.signing_secret.as_deref().filter(|s| s.len() >= 16) {
            Some(s) => s.as_bytes().to_vec(),
            None => reel_auth::load_or_create_signing_key(&config.data_dir.join("signing.key")),
        };
        let auth = AuthService::new(
            db.clone(),
            AuthOptions {
                session_ttl: Duration::from_secs(
                    u64::from(config.session_ttl_days.max(1)) * 86_400,
                ),
                ..Default::default()
            },
            key,
        );
        let iptv = reel_iptv::IptvService::new(
            db.clone(),
            config.data_dir.clone(),
            reel_iptv::IptvConfig {
                hub: reel_iptv::HubConfig {
                    grace: Duration::from_secs(config.iptv_grace_secs),
                    stall_after: Duration::from_secs(config.iptv_stall_secs.max(1)),
                    ffmpeg_path: config
                        .ffmpeg_path
                        .clone()
                        .unwrap_or_else(|| "ffmpeg".into()),
                    ..Default::default()
                },
                epg_days_forward: config.iptv_epg_days.max(1),
                warm_favorites: config.iptv_warm_favorites,
                allow_private_fetch: config.allow_private_fetch,
                allow_private_logo_fetch: config.allow_private_logo_fetch,
                ..Default::default()
            },
        );
        Self {
            vod: Arc::default(),
            cast: Arc::default(),
            ws: Arc::default(),
            tmdb: None,
            omdb: config
                .omdb_api_key
                .as_deref()
                .filter(|k| !k.trim().is_empty())
                .map(|k| reel_requests::reviews::OmdbClient::new(k.trim())),
            trakt: config
                .trakt_client_id
                .as_deref()
                .filter(|k| !k.trim().is_empty())
                .map(|k| reel_requests::reviews::TraktClient::new(k.trim())),
            iptv,
            auth,
            requests,
            playback,
            hwaccel: Arc::new(hw),
            db,
            scanner: Scanner::new()
                .with_events(events.clone())
                .with_rename_window(Duration::from_secs(20)),
            images,
            config: Arc::new(config),
            events,
            watcher: Arc::default(),
        }
    }

    /// Replace the auth options (session lifetime, rate-limit thresholds), keeping the signing key.
    pub fn with_auth_options(mut self, opts: AuthOptions) -> Self {
        self.auth = AuthService::new(self.db.clone(), opts, self.auth.signing_key().to_vec());
        self
    }

    /// Enable TMDB discovery/requests metadata (rebuilds the requests service; call before
    /// `spawn_tracker`).
    pub fn with_tmdb(mut self, tmdb: reel_metadata::TmdbClient) -> Self {
        let requests =
            RequestsService::new(self.db.clone(), Some(tmdb.clone()), self.events.clone());
        self.tmdb = Some(tmdb);
        self.with_requests(requests)
    }

    /// Override the OMDb / Trakt clients (tests; normally built from the config).
    pub fn with_review_sources(
        mut self,
        omdb: Option<reel_requests::reviews::OmdbClient>,
        trakt: Option<reel_requests::reviews::TraktClient>,
    ) -> Self {
        self.omdb = omdb;
        self.trakt = trakt;
        self
    }

    /// Use a pre-built requests service (custom tracker options, tests).
    pub fn with_requests(mut self, requests: RequestsService) -> Self {
        self.requests = requests;
        self
    }

    /// Use detected hardware acceleration (rebuilds the session manager).
    pub fn with_hwaccel(mut self, hw: HwAccelStatus) -> Self {
        self.playback = new_playback(&self.config, hw.selected.clone());
        // IPTV transcodes (non-H.264 sources, e.g. MPEG-2 SD) use the same hardware encoder.
        self.iptv.live.set_hwaccel(hw.selected.clone());
        self.hwaccel = Arc::new(hw);
        self
    }

    /// Stop a playback session and drop its DB row. False if unknown.
    pub async fn end_session(&self, id: &str) -> bool {
        let removed = self.playback.remove(id).await;
        self.vod.forget(id);
        self.iptv.live.vod_release_session(id);
        let _ = sqlx::query("DELETE FROM playback_sessions WHERE session_key = ?")
            .bind(id)
            .execute(&self.db)
            .await;
        removed
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

/// Session manager wired to the upstream proxy: ffmpeg/ffprobe fetch provider URLs only through
/// a loopback endpoint backed by the SSRF-guarded HTTP client.
fn new_playback(config: &reel_core::Config, hw: reel_playback::HwAccel) -> Arc<SessionManager> {
    let m = SessionManager::new(session_config(config), hw);
    let allow = config.allow_private_fetch;
    m.media()
        .set_upstream(reel_playback::upstream::UpstreamProxy::new(
            reel_iptv::net::client(allow),
            Arc::new(move |u: &str| reel_iptv::net::check_url(u, allow).map_err(|e| e.to_string())),
        ));
    m
}

fn session_config(config: &reel_core::Config) -> SessionConfig {
    let mut c = SessionConfig::new(config.data_dir.clone());
    if let Some(p) = &config.ffmpeg_path {
        c.ffmpeg_path = p.clone();
    }
    c.max_global_transcodes = config.max_transcodes;
    c.max_user_transcodes = config.max_user_transcodes;
    c
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
    modifiers(&SecurityAddon),
    security(("bearer" = [])),
    paths(playback::doc_stream_master, playback::doc_stream_file),
    components(schemas(ServerEvent, reel_core::RequestState, error::ErrorBody))
)]
struct ApiDoc;

/// Registers the `bearer` security scheme.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let c = openapi.components.get_or_insert_with(Default::default);
        c.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "Opaque session token from /auth/login. Media endpoints (direct, stream, \
                         subs, images) also accept a signed URL (`exp`, `u`, `sig` query).",
                    ))
                    .build(),
            ),
        );
    }
}

/// Health response.
#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: String,
    pub version: String,
    /// Git commit the server was built from (short SHA), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// Build timestamp (UTC, RFC 3339), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_date: Option<String>,
}

/// Build metadata set by the image (`REEL_GIT_SHA`, `REEL_BUILD_DATE` env vars in the runtime
/// layer, so a new commit doesn't invalidate the cached Rust build).
fn build_info(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Liveness check.
#[utoipa::path(get, path = "/health", security(()), responses((status = 200, body = Health)))]
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        git_sha: build_info("REEL_GIT_SHA").map(|s| s.chars().take(12).collect()),
        build_date: build_info("REEL_BUILD_DATE"),
    })
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct EventsQuery {
    /// Single-use ticket from `POST /auth/ws-ticket` (preferred; browsers cannot set headers on
    /// WebSocket upgrades).
    pub ticket: Option<String>,
    /// Deprecated: the session token in the URL (ends up in logs). Still accepted for one release.
    pub token: Option<String>,
}

/// WebSocket stream of [`ServerEvent`]s as JSON text frames. Authenticate with `?ticket=` (from
/// `POST /auth/ws-ticket`), `Authorization: Bearer`, or the deprecated `?token=`. The socket is
/// closed when the session is revoked or the user disabled; at most 8 sockets per user. `request_progress` is delivered to the requester (and
/// admins); `scan_progress` to admins only.
#[utoipa::path(
    get,
    path = "/events",
    params(EventsQuery),
    responses(
        (status = 101, description = "WebSocket upgrade; frames are JSON ServerEvent"),
        (status = 401, body = error::ErrorBody),
        (status = 429, description = "Too many open sockets", body = error::ErrorBody)
    )
)]
pub async fn events(
    ws: WebSocketUpgrade,
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
) -> Result<Response, error::ApiError> {
    let unauthorized = || error::ApiError::Unauthorized("authentication required".into());
    let (user, session_id) = if let Some(t) = q.ticket.as_deref() {
        let (uid, sid) = st.ws.redeem(t).ok_or_else(unauthorized)?;
        let user = st
            .auth
            .get_user(uid)
            .await?
            .filter(|u| !u.disabled)
            .ok_or_else(unauthorized)?;
        if !st.auth.session_valid(sid, uid).await {
            return Err(unauthorized());
        }
        (user, sid)
    } else {
        let token = auth::bearer(&headers)
            .map(str::to_string)
            .or_else(|| {
                q.token.inspect(|_| {
                    tracing::warn!(
                        "WebSocket authenticated with ?token= (deprecated, token visible in URLs); \
                         use POST /auth/ws-ticket and ?ticket="
                    )
                })
            })
            .ok_or_else(unauthorized)?;
        let (user, s) = st.auth.authenticate(&token).await?;
        (user, s.id)
    };
    let slot = st.ws.acquire(user.id).ok_or_else(|| {
        error::ApiError::TooManyRequests("too many open event streams for this account".into())
    })?;
    let rx = st.events.subscribe();
    Ok(ws.on_upgrade(move |socket| async move {
        forward_events(socket, rx, st, user, session_id).await;
        drop(slot);
    }))
}

/// May `user` see `ev`?
async fn event_visible(
    st: &AppState,
    user: &User,
    owners: &mut std::collections::HashMap<i64, Option<i64>>,
    ev: &ServerEvent,
) -> bool {
    if user.is_admin() {
        return true;
    }
    match ev {
        ServerEvent::ScanProgress { .. } => false,
        ServerEvent::RequestProgress { request_id, .. } => {
            let owner = match owners.get(request_id) {
                Some(o) => *o,
                None => {
                    let o: Option<i64> =
                        sqlx::query_scalar("SELECT user_id FROM requests WHERE id = ?")
                            .bind(request_id)
                            .fetch_optional(&st.db)
                            .await
                            .ok()
                            .flatten();
                    owners.insert(*request_id, o);
                    o
                }
            };
            owner == Some(user.id)
        }
    }
}

async fn forward_events(
    mut socket: WebSocket,
    mut rx: broadcast::Receiver<ServerEvent>,
    st: AppState,
    user: User,
    session_id: i64,
) {
    let mut owners = std::collections::HashMap::new();
    // Download activity of all Arr queues is pushed as `{"type":"downloads","items":[…]}`: every
    // 2 s while something is active, otherwise only a cheap check every 10 s. The snapshot is
    // cached by the service, so any number of sockets cost the Arrs one poll per 2 s.
    let mut dl_tick = tokio::time::interval(Duration::from_secs(2));
    dl_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut dl_active, mut dl_idle, mut dl_last) = (false, 0u32, String::from("[]"));
    // revoked / disabled accounts lose their socket within half a minute
    let mut alive_tick =
        tokio::time::interval(Duration::from_secs(st.config.ws_recheck_secs.max(1)));
    alive_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    alive_tick.tick().await;
    loop {
        tokio::select! {
            _ = alive_tick.tick() => {
                if !st.auth.session_valid(session_id, user.id).await {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            }
            _ = dl_tick.tick() => {
                if !dl_active {
                    dl_idle += 1;
                    if dl_idle % 5 != 1 {
                        continue;
                    }
                }
                let Ok(items) = st.requests.downloads().await else { continue };
                let dtos = extras::downloads_for(&st, &user, &items).await;
                dl_active = !dtos.is_empty();
                let Ok(items_json) = serde_json::to_string(&dtos) else { continue };
                if items_json != dl_last {
                    let text = format!("{{\"type\":\"downloads\",\"items\":{items_json}}}");
                    dl_last = items_json;
                    if socket.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
            },
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if !event_visible(&st, &user, &mut owners, &ev).await {
                        continue;
                    }
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
        .routes(routes!(auth::setup_status, auth::setup))
        .routes(routes!(auth::login))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(auth::my_sessions))
        .routes(routes!(auth::revoke_my_session))
        .routes(routes!(auth::change_password))
        .routes(routes!(auth::invite_info))
        .routes(routes!(auth::accept_invite))
        .routes(routes!(auth::list_users, auth::create_user))
        .routes(routes!(auth::get_user, auth::patch_user, auth::delete_user))
        .routes(routes!(auth::revoke_user_sessions))
        .routes(routes!(auth::list_invites, auth::create_invite))
        .routes(routes!(auth::delete_invite))
        .routes(routes!(auth::audit_log))
        .routes(routes!(iptv::list_channels))
        .routes(routes!(iptv::list_groups))
        .routes(routes!(iptv::epg_grid))
        .routes(routes!(iptv::now_next))
        .routes(routes!(iptv::tune))
        .routes(routes!(iptv::add_favorite, iptv::remove_favorite))
        .routes(routes!(iptv::stop_session))
        .routes(routes!(iptv::live_file))
        .routes(routes!(iptv::channel_logo))
        .routes(routes!(iptv::list_sources, iptv::create_source))
        .routes(routes!(iptv::test_new_source))
        .routes(routes!(
            iptv::get_source,
            iptv::update_source,
            iptv::delete_source
        ))
        .routes(routes!(iptv::refresh_source))
        .routes(routes!(iptv::refresh_source_epg))
        .routes(routes!(iptv::test_source))
        .routes(routes!(iptv::admin_channels))
        .routes(routes!(iptv::patch_channel))
        .routes(routes!(iptv::set_aliases))
        .routes(routes!(iptv::hub_status))
        .routes(routes!(iptv::admin_live_sessions))
        .routes(routes!(routes::list_libraries, routes::create_library))
        .routes(routes!(routes::scan_library))
        .routes(routes!(routes::library_items))
        .routes(routes!(routes::get_item))
        .routes(routes!(routes::item_children))
        .routes(routes!(routes::home))
        .routes(routes!(routes::search))
        .routes(routes!(routes::image))
        .routes(routes!(routes::direct))
        .routes(routes!(playback::decide_playback))
        .routes(routes!(
            playback::playback_progress,
            playback::stop_playback
        ))
        .routes(routes!(playback::subtitle_vtt))
        .routes(routes!(playback::admin_sessions))
        .routes(routes!(playback::admin_kill_session))
        .routes(routes!(playback::admin_hwaccel))
        .routes(routes!(playback::mark_played, playback::mark_unplayed))
        .routes(routes!(routes::image_tmdb))
        .routes(routes!(requests::discover_search))
        .routes(routes!(requests::discover_list))
        .routes(routes!(requests::discover_detail))
        .routes(routes!(requests::list_requests, requests::create_request))
        .routes(routes!(requests::active_requests))
        .routes(routes!(requests::approve_request))
        .routes(routes!(requests::decline_request))
        .routes(routes!(requests::retry_request))
        .routes(routes!(browse::list_genres))
        .routes(routes!(browse::browse_lanes))
        .routes(routes!(browse::pick))
        .routes(routes!(browse::search_all))
        .routes(routes!(vod::vod_categories))
        .routes(routes!(vod::vod_movies))
        .routes(routes!(vod::vod_series))
        .routes(routes!(vod::vod_movie))
        .routes(routes!(vod::vod_series_detail))
        .routes(routes!(vod::vod_lanes))
        .routes(routes!(vod::vod_play))
        .routes(routes!(vod::vod_direct))
        .routes(routes!(vod::vod_subs))
        .routes(routes!(prefs::get_preferences, prefs::put_preferences))
        .routes(routes!(prefs::cast_url))
        .routes(routes!(ws::ws_ticket))
        .routes(routes!(cast::list_devices))
        .routes(routes!(cast::add_device))
        .routes(routes!(cast::play_on_device))
        .routes(routes!(cast::get_session))
        .routes(routes!(cast::control))
        .routes(routes!(cast::set_volume))
        .routes(routes!(vod::vod_image_route))
        .routes(routes!(extras::get_person))
        .routes(routes!(extras::get_reviews))
        .routes(routes!(extras::list_downloads))
        .routes(routes!(requests::arr_webhook))
        .routes(routes!(
            requests::list_arr_servers,
            requests::create_arr_server
        ))
        .routes(routes!(
            requests::update_arr_server,
            requests::delete_arr_server
        ))
        .routes(routes!(requests::arr_profiles))
        .routes(routes!(
            requests::list_download_clients,
            requests::create_download_client
        ))
        .routes(routes!(requests::delete_download_client))
        .routes(routes!(
            requests::list_notifications,
            requests::create_notification
        ))
        .routes(routes!(requests::delete_notification))
        .routes(routes!(requests::test_notification))
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
    // TODO(auth): signed URLs for stream/direct/subs.
    let stream = reel_playback::router::stream_router(state.playback.clone()).layer(
        axum::middleware::from_fn_with_state(state.clone(), stream::stream_guard),
    );
    let cors = cors_layer(&state.config.cors_origins);
    let body_limit = state.config.max_body_bytes.max(1024);
    let router = match state.config.web_dir.clone() {
        Some(dir) => {
            let index: PathBuf = dir.join("index.html");
            let web = axum::Router::new()
                .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
                .layer(axum::middleware::from_fn(web_cache_headers));
            router.fallback_service(web)
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
        .with_state(state)
        .nest("/api/v1", stream)
        // spans carry the path only: query strings hold signatures, tokens and tickets
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                tracing::info_span!("request", method = %req.method(), path = %req.uri().path())
            }),
        )
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(tower::util::option_layer(cors))
        .layer(axum::middleware::from_fn(media_cors))
}

/// Signed media routes answer cross-origin requests from anywhere (Chromecast receivers, hls.js on
/// another origin): the URL's signature is the authorization, nothing ambient is exposed.
fn is_media_path(p: &str) -> bool {
    let p = p.strip_prefix("/api/v1").unwrap_or(p);
    [
        "/stream/",
        "/direct/",
        "/subs/",
        "/vod/direct/",
        "/vod/subs/",
        "/vod/images/",
        "/iptv/live/",
        "/iptv/logo/",
        "/images/",
    ]
    .iter()
    .any(|pre| p.starts_with(pre))
}

async fn media_cors(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let media = is_media_path(req.uri().path());
    if media && req.method() == Method::OPTIONS {
        return Response::builder()
            .status(axum::http::StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                "Range, Content-Type, Accept",
            )
            .header(header::ACCESS_CONTROL_MAX_AGE, "86400")
            .body(axum::body::Body::empty())
            .unwrap_or_default();
    }
    let mut res = next.run(req).await;
    if media
        && !res
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    {
        let h = res.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        h.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("Content-Range, Accept-Ranges, Content-Length, Content-Type"),
        );
    }
    res
}

/// Same-origin by default; with `cors_origins` set, those origins may call the API.
/// Cache policy for the bundled web client: Vite's hashed `/assets/*` are immutable; everything else
/// (index.html and the SPA fallback for deep links) must be revalidated so a deploy is picked up
/// immediately instead of a stale index.html pointing at assets that no longer exist.
async fn web_cache_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let immutable = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    if res.status().is_success() && !res.headers().contains_key(header::CACHE_CONTROL) {
        let v = if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        res.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(v));
    }
    res
}

fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o.trim_end_matches('/')).ok())
        .collect();
    (!list.is_empty()).then(|| {
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(list))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::RANGE])
            .expose_headers([
                header::CONTENT_RANGE,
                header::ACCEPT_RANGES,
                header::RETRY_AFTER,
            ])
            .max_age(Duration::from_secs(600))
    })
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
        let dir = std::env::temp_dir().join(format!("reel-health-{}", std::process::id()));
        let cfg = reel_core::Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let app = app(AppState::new(db, cfg));
        let res = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert!(v["version"].is_string());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
