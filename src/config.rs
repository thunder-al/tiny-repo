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
    /// Largest blob (one layer, not a whole image) accepted, as bytes or with a
    /// suffix (`500MiB`, `50GiB`, `2TB`). `0` turns the limit off.
    pub max_blob_size: ByteSize,
    /// An upload with no request for this long is dropped, with its staged file.
    /// A request in progress keeps it alive however long it takes.
    pub session_ttl_secs: u64,
}

/// A size in bytes, written as a number or a number with a unit suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ByteSize(pub u64);

impl std::str::FromStr for ByteSize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const UNITS: [(&str, u64); 9] = [
            ("KiB", 1 << 10),
            ("MiB", 1 << 20),
            ("GiB", 1 << 30),
            ("TiB", 1 << 40),
            ("KB", 1_000),
            ("MB", 1_000_000),
            ("GB", 1_000_000_000),
            ("TB", 1_000_000_000_000),
            ("B", 1),
        ];

        let s = s.trim();
        let (number, unit) = UNITS
            .iter()
            .find_map(|(suffix, unit)| s.strip_suffix(suffix).map(|n| (n.trim_end(), *unit)))
            .unwrap_or((s, 1));

        number
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(unit))
            .map(Self)
            .ok_or_else(|| format!("invalid size {s:?}, expected e.g. 10GiB, 500MB or 1048576"))
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // TOML and env give a number or a string; figment hands env values over
        // already parsed, so `0` arrives as an integer.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u64),
            Text(String),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Number(n) => Ok(Self(n)),
            Raw::Text(s) => s.parse().map_err(serde::de::Error::custom),
        }
    }
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
            uploads: UploadsConfig {
                dir: None,
                max_blob_size: ByteSize(10 << 30),
                session_ttl_secs: 3600,
            },
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
        assert_eq!(c.uploads.max_blob_size, ByteSize(10 << 30));
        assert_eq!(c.uploads.session_ttl_secs, 3600);
    }

    #[test]
    fn blob_size_limit_with_units_or_off() {
        for (value, bytes) in [
            ("\"50GiB\"", 50 << 30),
            ("\"500 MB\"", 500_000_000),
            ("\"2TB\"", 2_000_000_000_000),
            ("1048576", 1 << 20),
            ("0", 0),
        ] {
            let c = parse(&format!("[uploads]\nmax_blob_size = {value}")).unwrap();
            assert_eq!(c.uploads.max_blob_size, ByteSize(bytes), "{value}");
        }
        for value in ["\"10 GiBs\"", "\"-1GB\"", "\"lots\"", "\"99999999TiB\""] {
            assert!(
                parse(&format!("[uploads]\nmax_blob_size = {value}")).is_err(),
                "{value} should be rejected"
            );
        }
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
