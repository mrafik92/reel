//! Configuration: defaults < TOML file < `REEL_*` environment variables.

use std::path::{Path, PathBuf};

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use crate::Result;

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// Socket address to bind, e.g. `0.0.0.0:8420`.
    pub bind: String,
    /// Directory for the database, image cache, etc.
    pub data_dir: PathBuf,
    /// SQLite file path; defaults to `<data_dir>/reel.db`.
    pub database_path: Option<PathBuf>,
    /// TMDB API key (prefer the `REEL_TMDB_API_KEY` env var).
    pub tmdb_api_key: Option<String>,
    /// Directory with the built web client; served with SPA fallback when set.
    pub web_dir: Option<PathBuf>,
    /// Max concurrent transcode sessions overall (default 3; 0 = unlimited).
    pub max_transcodes: usize,
    /// Max concurrent transcode sessions per user (default 2; 0 = unlimited).
    pub max_user_transcodes: usize,
    /// ffmpeg binary used for transcoding/subtitles (default `ffmpeg`).
    pub ffmpeg_path: Option<String>,
    /// Shared secret for `POST /api/v1/webhooks/arr?token=…` (Radarr/Sonarr webhooks).
    /// Webhooks are rejected while this is unset.
    pub webhook_secret: Option<String>,
    /// HMAC key for signed media URLs. Default: generated once and kept in `<data_dir>/signing.key`.
    pub signing_secret: Option<String>,
    /// Sliding session lifetime in days (default 30).
    pub session_ttl_days: u32,
    /// Lifetime of signed stream/direct/subtitle URLs in seconds (default 6 h; must outlast a movie).
    pub signed_url_ttl_secs: u64,
    /// Origins allowed to call the API cross-origin (e.g. a separately hosted web client).
    /// Empty (default) = same-origin only.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Public base URL used in invite links, e.g. `https://reel.example.com`.
    pub public_url: Option<String>,
    /// Trust `X-Forwarded-For` / `X-Real-IP` for the client IP (set when behind Caddy/nginx).
    pub trust_proxy_headers: bool,
    /// Reverse proxies (CIDRs, e.g. `["172.18.0.0/16"]`) whose `X-Forwarded-For` is believed: the
    /// client is the right-most entry that is not itself a trusted proxy. Empty (default) = the
    /// headers are ignored. `trust_proxy_headers` is a deprecated alias meaning "trust private-range
    /// peers".
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Maximum request body size in bytes (default 4 MiB).
    pub max_body_bytes: usize,
    /// Keep a live channel's upstream open this many seconds after the last viewer leaves.
    pub iptv_grace_secs: u64,
    /// Seconds without upstream data before a live channel reconnects / fails over.
    pub iptv_stall_secs: u64,
    /// Keep the N most-favorited channels warm if the provider connection budget allows (0 = off).
    pub iptv_warm_favorites: usize,
    /// Days of EPG kept ahead of now (1 day is kept behind).
    pub iptv_epg_days: u32,
    /// Server-side fetches of admin-supplied URLs (IPTV playlists, EPG, Xtream APIs, stream
    /// upstreams) may reach private/LAN addresses. Default true: these are admin-trusted and often
    /// LAN-hosted. Set false to block loopback/private/link-local targets (SSRF hardening).
    pub allow_private_fetch: bool,
    /// Channel logos come from third-party playlists, so private/loopback targets are blocked
    /// unless this is true (default false).
    pub allow_private_logo_fetch: bool,
    /// Base URL TVs / Chromecasts use to reach this server (LAN address, e.g.
    /// `http://10.10.10.207:8420`). Used for cast URLs (`for_cast`, `POST /playback/{id}/cast-url`).
    pub cast_base_url: Option<String>,
    /// OMDb API key (`REEL_OMDB_API_KEY`): adds IMDb / Rotten Tomatoes / Metacritic scores.
    pub omdb_api_key: Option<String>,
    /// Trakt client id (`REEL_TRAKT_CLIENT_ID`): adds Trakt comments.
    pub trakt_client_id: Option<String>,
    /// Libraries created on boot if no library with the same name exists.
    #[serde(default)]
    pub libraries: Vec<LibraryConfig>,
}

/// Bootstrap library definition (`[[libraries]]` in the TOML file).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryConfig {
    pub name: String,
    /// `movies` or `shows`.
    pub kind: String,
    pub paths: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8420".into(),
            data_dir: PathBuf::from("./data"),
            database_path: None,
            tmdb_api_key: None,
            web_dir: None,
            max_transcodes: 3,
            max_user_transcodes: 2,
            ffmpeg_path: None,
            webhook_secret: None,
            signing_secret: None,
            session_ttl_days: 30,
            signed_url_ttl_secs: 6 * 3600,
            cors_origins: Vec::new(),
            public_url: None,
            trust_proxy_headers: false,
            trusted_proxies: Vec::new(),
            max_body_bytes: 4 * 1024 * 1024,
            iptv_grace_secs: 15,
            iptv_stall_secs: 8,
            iptv_warm_favorites: 0,
            iptv_epg_days: 7,
            allow_private_fetch: true,
            allow_private_logo_fetch: false,
            cast_base_url: None,
            omdb_api_key: None,
            trakt_client_id: None,
            libraries: Vec::new(),
        }
    }
}

impl Config {
    /// Load defaults, then the optional TOML file, then `REEL_*` env vars.
    pub fn load(file: Option<&Path>) -> Result<Self> {
        let mut fig = Figment::from(Serialized::defaults(Config::default()));
        if let Some(f) = file {
            fig = fig.merge(Toml::file(f));
        }
        Ok(fig.merge(Env::prefixed("REEL_")).extract()?)
    }

    /// Effective database path.
    pub fn database_path(&self) -> PathBuf {
        self.database_path
            .clone()
            .unwrap_or_else(|| self.data_dir.join("reel.db"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load() {
        figment::Jail::expect_with(|_| {
            let c = Config::load(None).unwrap();
            assert_eq!(c.bind, "0.0.0.0:8420");
            assert_eq!(c.database_path(), PathBuf::from("./data/reel.db"));
            assert!(c.tmdb_api_key.is_none());
            Ok(())
        });
    }

    #[test]
    fn file_and_env_override() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("c.toml", "bind = \"127.0.0.1:1\"\ndata_dir = \"/x\"")?;
            jail.set_env("REEL_BIND", "127.0.0.1:2");
            let c = Config::load(Some(Path::new("c.toml"))).unwrap();
            assert_eq!(c.bind, "127.0.0.1:2");
            assert_eq!(c.database_path(), PathBuf::from("/x/reel.db"));
            Ok(())
        });
    }
}
