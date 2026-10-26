//! Configuration: defaults, then `tiny-repo.toml` (or `--config`), then
//! `TINY_REPO_*` environment variables. Unknown keys are an error, so typos fail
//! at startup instead of being silently ignored.

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// `tracing` filter, e.g. `info` or `tiny_repo=debug,tower_http=info`.
    pub level: String,
    pub format: LogFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Text,
    Json,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8000)),
            log: LogConfig {
                level: "info".into(),
                format: LogFormat::Text,
            },
        }
    }
}

impl Config {
    /// `TINY_REPO_LISTEN`, `TINY_REPO_LOG__LEVEL`, `TINY_REPO_LOG__FORMAT`: a double
    /// underscore separates nested keys. `TINY_REPO_CONFIG` (the file path, read by
    /// the CLI) is not a config key.
    pub fn load(file: &Path) -> Result<Self, Box<figment::Error>> {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(file))
            .merge(Env::prefixed("TINY_REPO_").ignore(&["config"]).split("__"))
            .extract()
            .map_err(Box::new)
    }
}
