//! Configuration: defaults, then `tiny-repo.toml` (or `--config`), then
//! `TINY_REPO_*` environment variables. Unknown keys are an error, so typos fail
//! at startup instead of being silently ignored.

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::Path, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    pub log: LogConfig,
    /// Where blobs live. Left out of the merged defaults: a section is taken whole
    /// or not at all, so switching `type` never inherits keys of the default type.
    #[serde(default, skip_serializing)]
    pub storage: StorageConfig,
    /// Where meta (repositories, manifests, tags, users, tokens) lives.
    #[serde(default, skip_serializing)]
    pub meta: MetaConfig,
    pub uploads: UploadsConfig,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum StorageConfig {
    /// Local directory; on Fly, a volume.
    Fs { root: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum MetaConfig {
    /// The meta backend that comes with `storage`: for `fs`, `<root>/meta`.
    // A struct variant, not a unit one: serde checks unknown keys only on those.
    Storage {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadsConfig {
    /// Staging directory for uploads in progress. Default for `fs` storage:
    /// `<root>/uploads`, which keeps it on the same filesystem as the blobs.
    pub dir: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self::Fs {
            root: PathBuf::from("data"),
        }
    }
}

impl Default for MetaConfig {
    fn default() -> Self {
        Self::Storage {}
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8000)),
            log: LogConfig {
                level: "info".into(),
                format: LogFormat::Text,
            },
            storage: StorageConfig::default(),
            meta: MetaConfig::default(),
            uploads: UploadsConfig { dir: None },
        }
    }
}

impl Config {
    /// `TINY_REPO_LISTEN`, `TINY_REPO_LOG__LEVEL`, `TINY_REPO_STORAGE__ROOT`, ...: a
    /// double underscore separates nested keys. `TINY_REPO_CONFIG` (the file path,
    /// read by the CLI) is not a config key.
    pub fn load(file: &Path) -> Result<Self, Box<figment::Error>> {
        Self::defaults()
            .merge(Toml::file(file))
            .merge(Env::prefixed("TINY_REPO_").ignore(&["config"]).split("__"))
            .extract()
            .map_err(Box::new)
    }

    fn defaults() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> Result<Config, Box<figment::Error>> {
        Config::defaults()
            .merge(Toml::string(toml))
            .extract()
            .map_err(Box::new)
    }

    #[test]
    fn defaults_are_fs_with_meta_and_uploads_beside_it() {
        let c = parse("").unwrap();
        assert_eq!(
            c.storage,
            StorageConfig::Fs {
                root: "data".into()
            }
        );
        assert_eq!(c.meta, MetaConfig::Storage {});
        assert_eq!(c.uploads.dir, None);
    }

    #[test]
    fn storage_root_and_uploads_dir() {
        let c = parse(
            r#"
            [storage]
            type = "fs"
            root = "/data"

            [uploads]
            dir = "/scratch"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.storage,
            StorageConfig::Fs {
                root: "/data".into()
            }
        );
        assert_eq!(c.meta, MetaConfig::Storage {});
        assert_eq!(c.uploads.dir, Some("/scratch".into()));
    }

    #[test]
    fn rejects_unknown_types_and_keys() {
        for toml in [
            "[storage]\ntype = \"s4\"",
            "[storage]\ntype = \"fs\"\nroot = \"/d\"\nbucket = \"x\"",
            "[meta]\ntype = \"postgres\"",
            "[meta]\ntype = \"storage\"\nurl = \"x\"",
            "[uploads]\ndirectory = \"/x\"",
            // A section without its type: nothing is inherited from the default.
            "[storage]\nroot = \"/d\"",
        ] {
            assert!(parse(toml).is_err(), "{toml:?} should be rejected");
        }
    }
}
