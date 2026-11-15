use std::path::PathBuf;

use std::sync::Arc;

use reel_api::{AppState, app, bootstrap_libraries, enrich::EnrichQueue};
use reel_core::{Config, db};
use reel_metadata::{Enricher, TmdbClient};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=debug".into()),
        )
        .init();

    let cfg_path = PathBuf::from(std::env::var("REEL_CONFIG").unwrap_or("config.toml".into()));
    let config = Config::load(cfg_path.exists().then_some(cfg_path.as_path()))?;
    // Owner-only data dir (0700): the database, signing key and caches live there (#14).
    reel_core::fsperm::ensure_private_dir(&config.data_dir)?;
    log_proxy_trust(&config);
    let pool = db::connect(&config.database_path()).await?;

    let hw = reel_playback::detect().await;
    tracing::info!(selected = hw.selected.name(), "hardware acceleration");
    let mut state = AppState::new(pool.clone(), config.clone()).with_hwaccel(hw);
    state.playback.spawn_reaper();
    if let Some(s) = state
        .config
        .webhook_secret
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        if s.len() < 32 {
            tracing::warn!(
                "webhook_secret is shorter than 32 characters: Radarr/Sonarr webhooks are DISABLED \
                 until it is longer (generate one with `openssl rand -hex 32`)"
            );
        }
    }
    // signed-URL revocation epochs survive restarts
    state.auth.load_epochs().await?;
    // first boot: only the LAN (or whoever has this token) may claim the server
    if let Some(t) = state.auth.ensure_setup_token().await? {
        tracing::warn!(
            "FIRST-RUN SETUP is open. Create the admin account from the local network, or from \
             anywhere with this one-time token: {t}"
        );
    }
    state.cast.spawn_discovery(
        state.config.data_dir.clone(),
        state
            .config
            .cast_discovery
            .then(|| reel_api::cast::SSDP_ADDR.parse().expect("ssdp addr")),
    );
    // Stale rows from a previous run (sessions don't survive restarts).
    sqlx::query("DELETE FROM playback_sessions")
        .execute(&pool)
        .await?;
    // Local `.nfo` / artwork enrichment always runs; TMDB is layered on top when a key is set.
    let enricher = match config
        .tmdb_api_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
    {
        Some(key) => {
            let tmdb = TmdbClient::new(key);
            state = state.with_tmdb(tmdb.clone());
            Enricher::new(tmdb)
        }
        None => {
            tracing::info!("no TMDB API key configured; using local .nfo/artwork metadata only");
            Enricher::local()
        }
    };
    let queue = EnrichQueue::spawn(pool, Arc::new(enricher), Some(state.requests.clone()));
    state = state.with_metadata(queue);
    if state.auth.needs_setup().await? {
        tracing::warn!(
            "no admin account yet: open the web UI (or POST /api/v1/setup) to create one"
        );
    }
    state.requests.spawn_tracker();
    state.iptv.spawn_background();
    state.spawn_vod_reaper();
    state.restart_watcher().await;
    bootstrap_libraries(&state).await;

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("reel-server listening on {}", listener.local_addr()?);
    let playback = state.playback.clone();
    let iptv = state.iptv.clone();
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    playback.shutdown().await;
    iptv.shutdown().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
            }
            Err(_) => ctrl_c.await,
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

/// Say whose `X-Forwarded-For` is believed. The client IP drives the login lockout and the LAN-only
/// first-run setup, so a typo or the blanket alias should be visible in the log.
fn log_proxy_trust(c: &Config) {
    for bad in c
        .trusted_proxies
        .iter()
        .filter(|s| reel_api::netutil::Cidr::parse(s).is_none())
    {
        tracing::warn!(entry = %bad, "trusted_proxies: not an IP address or CIDR, ignored");
    }
    if c.trust_proxy_headers {
        tracing::warn!(
            "REEL_TRUST_PROXY_HEADERS=true believes X-Forwarded-For from ANY private-range peer; \
             set REEL_TRUSTED_PROXIES to your reverse proxy's address instead"
        );
    } else if !c.trusted_proxies.is_empty() {
        tracing::info!(proxies = ?c.trusted_proxies, "client IP taken from X-Forwarded-For of these proxies");
    }
}
