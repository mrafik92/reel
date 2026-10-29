use std::path::PathBuf;

use reel_api::{AppState, app};
use reel_core::{Config, db};
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

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("reel-server listening on {}", listener.local_addr()?);
    axum::serve(listener, app(AppState::new(pool, config)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
