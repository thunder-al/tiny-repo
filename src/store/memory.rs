//! In-memory backend for tests: the reference the conformance suite is checked
//! against, and a fast store for tests of the layers above.

use super::index::{IndexStorage, IndexedMeta};
use super::{BlobEntry, BlobRead, BlobStore, ByteRange, StagedFile, StoreError, UnixTime};
use crate::oci::Digest;
use bytes::Bytes;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::Mutex;

#[derive(Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<BTreeMap<Digest, (Bytes, UnixTime)>>,
}

impl MemoryBlobStore {
    fn blobs(&self) -> std::sync::MutexGuard<'_, BTreeMap<Digest, (Bytes, UnixTime)>> {
        // A panic while holding the lock already failed the test that caused it.
        self.blobs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BlobStore for MemoryBlobStore {
    async fn stat(&self, digest: &Digest) -> Result<Option<u64>, StoreError> {
        Ok(self.blobs().get(digest).map(|(data, _)| data.len() as u64))
    }

    async fn get(
        &self,
        digest: &Digest,
        range: Option<ByteRange>,
    ) -> Result<Option<BlobRead>, StoreError> {
        let Some(data) = self.blobs().get(digest).map(|(d, _)| d.clone()) else {
            return Ok(None);
        };

        let size = data.len() as u64;
        let body = match range {
            Some(range) => {
                let (start, end) = range.resolve(size)?;
                // Both bounds are within `data`, whose length fits in usize.
                data.slice(start as usize..=end as usize)
            }
            None => data,
        };

        Ok(Some(BlobRead {
            size,
            body: Box::new(Cursor::new(body)),
        }))
    }

    async fn delete(&self, digest: &Digest) -> Result<bool, StoreError> {
        Ok(self.blobs().remove(digest).is_some())
    }

    async fn list(
        &self,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Vec<BlobEntry>, StoreError> {
        Ok(self
            .blobs()
            .iter()
            .filter(|(d, _)| after.is_none_or(|a| *d > a))
            .take(limit)
            .map(|(d, (data, modified))| BlobEntry {
                digest: *d,
                size: data.len() as u64,
                modified: *modified,
            })
            .collect())
    }

    async fn put(&self, digest: &Digest, file: StagedFile) -> Result<(), StoreError> {
        let data = Bytes::from(tokio::fs::read(file.path()).await?);
        self.blobs()
            .entry(*digest)
            .or_insert_with(|| (data, UnixTime::now()));
        Ok(())
    }
}

pub type MemoryMeta = IndexedMeta<MemoryBodies>;

/// Manifest bodies in a map; the index itself only lives in the `IndexedMeta`.
#[derive(Default)]
pub struct MemoryBodies {
    bodies: Mutex<HashMap<Digest, Bytes>>,
}

impl MemoryBodies {
    fn bodies(&self) -> std::sync::MutexGuard<'_, HashMap<Digest, Bytes>> {
        self.bodies.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl IndexStorage for MemoryBodies {
    async fn save(&self, _index: &super::index::Index) -> Result<(), StoreError> {
        Ok(())
    }

    async fn put_body(&self, digest: &Digest, body: &Bytes) -> Result<(), StoreError> {
        self.bodies().entry(*digest).or_insert_with(|| body.clone());
        Ok(())
    }

    async fn get_body(&self, digest: &Digest) -> Result<Option<Bytes>, StoreError> {
        Ok(self.bodies().get(digest).cloned())
    }

    async fn delete_body(&self, digest: &Digest) -> Result<(), StoreError> {
        self.bodies().remove(digest);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryBlobStore, MemoryMeta};
    use crate::store::conformance::{Blobs, Held};

    crate::store::conformance::blob_store_suite!(Blobs::new(MemoryBlobStore::default()).await);
    crate::store::conformance::meta_suite!(Held::new(MemoryMeta::default()));
}
