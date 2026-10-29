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
