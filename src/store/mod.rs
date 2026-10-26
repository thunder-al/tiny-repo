//! Storage contract. `BlobStore` keeps content-addressed bytes; the meta traits keep
//! everything else (repositories, manifests, tags, users, tokens). Backends implement
//! them and are checked by the shared suite in `conformance`.

mod blob;
mod meta;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod memory;

pub use blob::{BlobEntry, BlobRead, BlobStore, ByteRange, UploadEntry};
pub use meta::{
    AuthMeta, BlobRef, Manifest, ManifestSummary, NewManifest, RegistryMeta, RepoSummary, TagEntry,
    TokenHash, TokenId, TokenRecord, User, UserName,
};

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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Seconds since the Unix epoch. Stored in meta records; the UI formats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixTime(pub u64);

impl UnixTime {
    pub fn now() -> Self {
        // A clock before 1970 is not a case worth an error path: treat it as the epoch.
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        )
    }
}
