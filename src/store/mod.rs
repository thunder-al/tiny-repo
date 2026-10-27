//! Storage contract. `BlobStore` keeps content-addressed bytes; the meta traits keep
//! everything else (repositories, manifests, tags, users, tokens). Uploads are staged
//! in a local directory (`Staging`) whatever the backend. Backends implement the
//! traits and are checked by the shared suite in `conformance`.

mod backend;
mod blob;
mod fs;
mod index;
mod meta;
mod staging;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod memory;

pub use backend::{Storage, open};
pub use blob::{BlobEntry, BlobRead, BlobStore, ByteRange};
pub use meta::{
    AuthMeta, BlobRef, Manifest, ManifestSummary, NewManifest, RegistryMeta, RepoSummary, TagEntry,
    TokenHash, TokenId, TokenRecord, User, UserName,
};
pub use staging::{StagedFile, Staging};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("upload session not found")]
    UploadUnknown,
    #[error("requested range not satisfiable")]
    RangeNotSatisfiable,
    #[error("already exists")]
    AlreadyExists,
    #[error("user not found")]
    UserUnknown,
    #[error("{}: {reason}", path.display())]
    Corrupt { path: PathBuf, reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Seconds since the Unix epoch. Stored in meta records; the UI formats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixTime(pub u64);

impl UnixTime {
    pub fn now() -> Self {
        Self::from_system(SystemTime::now())
    }

    pub fn from_system(t: SystemTime) -> Self {
        // A clock before 1970 is not a case worth an error path: treat it as the epoch.
        Self(t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }
}
