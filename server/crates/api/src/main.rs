use std::path::PathBuf;

use std::sync::Arc;

use reel_api::{AppState, app, bootstrap_libraries, enrich::EnrichQueue};
use reel_core::{Config, db};
use reel_metadata::{TmdbClient, TmdbEnricher};
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
    let pool = db::connect(&config.database_path()).await?;

    let hw = reel_playback::detect().await;
    tracing::info!(selected = hw.selected.name(), "hardware acceleration");
    let mut state = AppState::new(pool.clone(), config.clone()).with_hwaccel(hw);
    state.playback.spawn_reaper();
    // Stale rows from a previous run (sessions don't survive restarts).
    sqlx::query("DELETE FROM playback_sessions")
        .execute(&pool)
        .await?;
    match config
        .tmdb_api_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
    {
        Some(key) => {
            let tmdb = TmdbClient::new(key);
            state = state.with_tmdb(tmdb.clone());
            let enricher = Arc::new(TmdbEnricher::new(tmdb));
            let queue = EnrichQueue::spawn(pool, enricher, Some(state.requests.clone()));
            state = state.with_metadata(queue);
        }
        None => tracing::info!("no TMDB API key configured; metadata enrichment disabled"),
    }
    if state.auth.needs_setup().await? {
        tracing::warn!(
            "no admin account yet: open the web UI (or POST /api/v1/setup) to create one"
        );
    }
    state.requests.spawn_tracker();
    state.iptv.spawn_background();
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
