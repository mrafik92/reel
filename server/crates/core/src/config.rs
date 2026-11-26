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
    /// Empty (default) = same-origin only. From the environment: `REEL_CORS_ORIGINS=a,b`.
    #[serde(default, deserialize_with = "list_or_csv")]
    pub cors_origins: Vec<String>,
    /// Public base URL used in invite links, e.g. `https://reel.example.com`.
    pub public_url: Option<String>,
    /// Trust `X-Forwarded-For` / `X-Real-IP` for the client IP (set when behind Caddy/nginx).
    pub trust_proxy_headers: bool,
    /// Seconds between session re-validation of open `/events` sockets (default 30).
    pub ws_recheck_secs: u64,
    /// Seconds between WebSocket pings on open `/events` sockets (default 25), so idle proxy, NAT
    /// and VPN paths don't drop them silently.
    pub ws_ping_secs: u64,
    /// Reverse proxies (CIDRs, e.g. `["172.18.0.0/16"]`) whose `X-Forwarded-For` is believed: the
    /// client is the right-most entry that is not itself a trusted proxy. Empty (default) = the
    /// headers are ignored. `trust_proxy_headers` is a deprecated alias meaning "trust private-range
    /// peers". From the environment: `REEL_TRUSTED_PROXIES=10.10.10.73` (comma-separated for more).
    #[serde(default, deserialize_with = "list_or_csv")]
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
    /// Disk budget of the TMDB image cache (`<data_dir>/images`, originals and resized copies) in
    /// MiB (default 4096; 0 = unlimited). The least recently used files are deleted beyond it.
    pub image_cache_max_mb: u64,
    /// Disk budget shared by the IPTV logo and VOD image caches (`<data_dir>/iptv/{logos,
    /// vod-images}`) in MiB (default 1024; 0 = unlimited).
    pub iptv_image_cache_max_mb: u64,
    /// Base URL TVs / Chromecasts use to reach this server (LAN address, e.g.
    /// `http://10.10.10.207:8420`). Used for cast URLs (`for_cast`, `POST /playback/{id}/cast-url`).
    pub cast_base_url: Option<String>,
    /// `REEL_CAST_LAN_NETWORKS`: senders (the phone or laptop that starts a Chromecast / AirPlay
    /// cast) on these networks get cast URLs on `cast_base_url`; everyone else gets them on
    /// `public_url`, because their TV sits next to them, not next to the server (#309). CIDRs or
    /// addresses, comma-separated; a `!` entry excludes, e.g. `10.10.10.0/24,!10.10.10.1` when
    /// Tailscale clients arrive routed through the gateway. Empty (default) = private ranges
    /// except Tailscale's.
    #[serde(default, deserialize_with = "list_or_csv")]
    pub cast_lan_networks: Vec<String>,
    /// `REEL_CAST_DISCOVERY`: look for DLNA TVs with SSDP multicast (default true). Set false
    /// to only use TVs added by IP (Settings or `POST /admin/cast/devices`): SSDP answers can
    /// come from any host on the LAN.
    pub cast_discovery: bool,
    /// `REEL_ALLOW_DIRECT_LIVE`: let clients play Live TV straight from the provider instead of
    /// through Reel. Off by default: the tune response then carries the provider URL, which
    /// contains the provider login.
    pub allow_direct_live: bool,
    /// `REEL_REQUIRE_ADMIN_2FA`: an admin needs a second factor (passkey or authenticator app) for
    /// admin powers (#146). Default on; `false` is a recovery lever.
    #[serde(deserialize_with = "bool_or_num")]
    pub require_admin_2fa: bool,
    /// `REEL_ABR`: multi-variant HLS for players that switch variants themselves (#111): the
    /// original plus lazily transcoded rungs. Default on; `false`/`0` serves one rendition per
    /// session everywhere (a rollback lever without a rebuild).
    #[serde(deserialize_with = "bool_or_num")]
    pub abr: bool,
    /// `REEL_ABR_VARIANT_IDLE_SECS`: a variant nobody fetched for this long has its ffmpeg stopped
    /// and its transcode slot released (default 30).
    pub abr_variant_idle_secs: u64,
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
            ws_recheck_secs: 30,
            ws_ping_secs: 25,
            trusted_proxies: Vec::new(),
            max_body_bytes: 4 * 1024 * 1024,
            iptv_grace_secs: 15,
            iptv_stall_secs: 8,
            iptv_warm_favorites: 0,
            iptv_epg_days: 7,
            allow_private_fetch: true,
            allow_private_logo_fetch: false,
            image_cache_max_mb: 4096,
            iptv_image_cache_max_mb: 1024,
            cast_base_url: None,
            cast_lan_networks: Vec::new(),
            cast_discovery: true,
            allow_direct_live: false,
            require_admin_2fa: true,
            abr: true,
            abr_variant_idle_secs: 30,
            omdb_api_key: None,
            trakt_client_id: None,
            libraries: Vec::new(),
        }
    }
}

/// A bool from TOML/env that also accepts `0`/`1` (figment hands `REEL_ABR=0` over as a number).
fn bool_or_num<'de, D>(d: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum B {
        Bool(bool),
        Num(i64),
        Str(String),
    }
    match B::deserialize(d)? {
        B::Bool(b) => Ok(b),
        B::Num(n) => Ok(n != 0),
        B::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(serde::de::Error::custom(format!("not a bool: {other}"))),
        },
    }
}

/// A list from TOML (`["a", "b"]`) or from one environment variable (`a,b` or `a b`).
///
/// figment hands a plain `REEL_TRUSTED_PROXIES=10.10.10.73` over as a string, and a `Vec` field
/// would refuse it, so the server would not start with the obvious spelling.
fn list_or_csv<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ListOrCsv {
        List(Vec<String>),
        Csv(String),
    }
    Ok(match ListOrCsv::deserialize(d)? {
        ListOrCsv::List(v) => v,
        ListOrCsv::Csv(s) => s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    })
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
            assert_eq!(c.image_cache_max_mb, 4096);
            assert!(c.cast_discovery);
            assert!(c.abr);
            assert_eq!(c.abr_variant_idle_secs, 30);
            Ok(())
        });
    }

    #[test]
    fn abr_switch_accepts_numbers_and_words() {
        for (v, want) in [("0", false), ("1", true), ("false", false), ("true", true)] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("REEL_ABR", v);
                jail.set_env("REEL_ABR_VARIANT_IDLE_SECS", "45");
                let c = Config::load(None).unwrap();
                assert_eq!(c.abr, want, "REEL_ABR={v}");
                assert_eq!(c.abr_variant_idle_secs, 45);
                Ok(())
            });
        }
    }

    #[test]
    fn file_and_env_override() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("c.toml", "bind = \"127.0.0.1:1\"\ndata_dir = \"/x\"")?;
            jail.set_env("REEL_BIND", "127.0.0.1:2");
            jail.set_env("REEL_IMAGE_CACHE_MAX_MB", "512");
            jail.set_env("REEL_CAST_DISCOVERY", "false");
            let c = Config::load(Some(Path::new("c.toml"))).unwrap();
            assert_eq!(c.bind, "127.0.0.1:2");
            assert_eq!(c.image_cache_max_mb, 512);
            assert!(!c.cast_discovery);
            assert_eq!(c.database_path(), PathBuf::from("/x/reel.db"));
            Ok(())
        });
    }

    #[test]
    fn proxy_and_cors_lists_from_env() {
        let load = |k: &str, v: &str| {
            let mut out = None;
            figment::Jail::expect_with(|jail| {
                jail.set_env(k, v);
                out = Some(Config::load(None).unwrap());
                Ok(())
            });
            out.unwrap()
        };
        assert!(!load("REEL_REQUIRE_ADMIN_2FA", "false").require_admin_2fa);
        assert!(!load("REEL_REQUIRE_ADMIN_2FA", "0").require_admin_2fa);
        // the plain spelling used to stop the server from starting
        assert_eq!(
            load("REEL_TRUSTED_PROXIES", "10.10.10.73").trusted_proxies,
            vec!["10.10.10.73"]
        );
        assert_eq!(
            load("REEL_TRUSTED_PROXIES", "10.10.10.73/32, 172.18.0.0/16").trusted_proxies,
            vec!["10.10.10.73/32", "172.18.0.0/16"]
        );
        // the TOML-array spelling keeps working
        assert_eq!(
            load("REEL_TRUSTED_PROXIES", "[10.10.10.73, \"fd00::/8\"]").trusted_proxies,
            vec!["10.10.10.73", "fd00::/8"]
        );
        assert!(load("REEL_TRUSTED_PROXIES", "").trusted_proxies.is_empty());
        assert_eq!(
            load("REEL_CAST_LAN_NETWORKS", "10.10.10.0/24,!10.10.10.1").cast_lan_networks,
            vec!["10.10.10.0/24", "!10.10.10.1"]
        );
        assert_eq!(
            load("REEL_CORS_ORIGINS", "https://a.example,https://b.example").cors_origins,
            vec!["https://a.example", "https://b.example"]
        );
    }

    #[test]
    fn proxy_list_from_toml() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("c.toml", "trusted_proxies = [\"10.10.10.73\"]")?;
            let c = Config::load(Some(Path::new("c.toml"))).unwrap();
            assert_eq!(c.trusted_proxies, vec!["10.10.10.73"]);
            Ok(())
        });
    }
}
