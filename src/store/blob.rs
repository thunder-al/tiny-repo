//! Blob storage: bytes addressed by digest, one global pile shared by all
//! repositories. Which repository may read a blob is decided by the meta
//! (`RegistryMeta::repo_has_blob`), not here.

use super::{StoreError, UnixTime};
use crate::oci::{Digest, UploadId};
use std::time::Duration;
use tokio::io::AsyncRead;

pub type BlobReader = Box<dyn AsyncRead + Send + Unpin>;

/// Inclusive byte range, as in an HTTP `Range: bytes=<start>-<end>` header;
/// `end: None` reads to the end of the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: Option<u64>,
}

impl ByteRange {
    /// Clamps the range to a blob of `size` bytes and returns inclusive `(start, end)`.
    /// An end past the blob is cut off; a start past it cannot be served.
    pub fn resolve(self, size: u64) -> Result<(u64, u64), StoreError> {
        let last = size.checked_sub(1).ok_or(StoreError::RangeNotSatisfiable)?;
        let end = self.end.map_or(last, |end| end.min(last));

        if self.start > end {
            return Err(StoreError::RangeNotSatisfiable);
        }
        Ok((self.start, end))
    }
}

pub struct BlobRead {
    /// Size of the whole blob, also when a range was requested.
    pub size: u64,
    pub body: BlobReader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobEntry {
    pub digest: Digest,
    pub size: u64,
    pub modified: UnixTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadEntry {
    pub id: UploadId,
    pub started: UnixTime,
}

/// The store trusts the caller with digests: the core hashes the upload while it
/// streams and calls `commit` only after the digest matched.
pub trait BlobStore {
    /// Size of the blob, `None` if it is not stored.
    async fn stat(&self, digest: &Digest) -> Result<Option<u64>, StoreError>;

    async fn get(
        &self,
        digest: &Digest,
        range: Option<ByteRange>,
    ) -> Result<Option<BlobRead>, StoreError>;

    /// A URL the client can download the blob from directly. `None` means the store
    /// cannot sign URLs and the server streams the blob itself.
    #[allow(dead_code)] // called by the pull route (THL-39) and overridden by S3 (THL-40)
    async fn presign_get(
        &self,
        _digest: &Digest,
        _ttl: Duration,
    ) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    /// Returns whether the blob existed. Only GC deletes blobs.
    async fn delete(&self, digest: &Digest) -> Result<bool, StoreError>;

    /// Blobs ordered by digest, starting after `after`.
    async fn list(
        &self,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Vec<BlobEntry>, StoreError>;

    /// Fails with `AlreadyExists` if the id is taken.
    async fn start_upload(&self, id: &UploadId) -> Result<(), StoreError>;

    /// Appends the whole body to the upload and returns the number of bytes written.
    /// After an error the upload's content is undefined; the caller aborts it.
    async fn append(
        &self,
        id: &UploadId,
        body: impl AsyncRead + Send + Unpin,
    ) -> Result<u64, StoreError>;

    /// Moves the upload into the blob `digest` and ends the session. If the blob is
    /// already stored, the upload is dropped and the stored blob is kept.
    async fn commit(&self, id: &UploadId, digest: &Digest) -> Result<(), StoreError>;

    /// Returns whether the upload existed.
    async fn abort(&self, id: &UploadId) -> Result<bool, StoreError>;

    /// Unfinished uploads, for cleaning up stale ones.
    async fn list_uploads(&self) -> Result<Vec<UploadEntry>, StoreError>;
}
