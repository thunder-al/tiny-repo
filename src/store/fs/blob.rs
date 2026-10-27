use super::sync_parent;
use crate::oci::Digest;
use crate::store::{BlobEntry, BlobRead, BlobStore, ByteRange, StagedFile, StoreError, UnixTime};
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const ALGORITHMS: [&str; 2] = ["sha256", "sha512"];

/// Blobs as plain files, `<root>/blobs/<alg>/<hex>`. A blob file appears in one
/// rename, so readers never see a partial blob.
pub struct FsBlobStore {
    dir: PathBuf,
}

impl FsBlobStore {
    /// `root` is the storage root; blobs go to `root/blobs`.
    pub async fn open(root: &Path) -> Result<Self, StoreError> {
        let dir = root.join("blobs");
        for alg in ALGORITHMS {
            fs::create_dir_all(dir.join(alg)).await?;
        }
        Ok(Self { dir })
    }

    fn path(&self, digest: &Digest) -> PathBuf {
        self.dir.join(digest.algorithm()).join(digest.hex())
    }
}

fn none_if_missing<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

impl BlobStore for FsBlobStore {
    async fn stat(&self, digest: &Digest) -> Result<Option<u64>, StoreError> {
        Ok(none_if_missing(fs::metadata(self.path(digest)).await)?.map(|m| m.len()))
    }

    async fn get(
        &self,
        digest: &Digest,
        range: Option<ByteRange>,
    ) -> Result<Option<BlobRead>, StoreError> {
        let Some(mut file) = none_if_missing(File::open(self.path(digest)).await)? else {
            return Ok(None);
        };
        let size = file.metadata().await?.len();

        let Some(range) = range else {
            return Ok(Some(BlobRead {
                size,
                body: Box::new(file),
            }));
        };
        let (start, end) = range.resolve(size)?;
        file.seek(SeekFrom::Start(start)).await?;

        Ok(Some(BlobRead {
            size,
            body: Box::new(file.take(end - start + 1)),
        }))
    }

    async fn delete(&self, digest: &Digest) -> Result<bool, StoreError> {
        let path = self.path(digest);
        if crate::store::staging::remove_if_exists(&path).await? {
            sync_parent(&path).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn list(
        &self,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Vec<BlobEntry>, StoreError> {
        let mut digests = Vec::new();
        for alg in ALGORITHMS {
            let mut entries = fs::read_dir(self.dir.join(alg)).await?;
            while let Some(entry) = entries.next_entry().await? {
                let name = entry.file_name();
                match format!("{alg}:{}", name.to_string_lossy()).parse::<Digest>() {
                    Ok(d) => digests.push(d),
                    Err(_) => tracing::warn!(path = %entry.path().display(), "not a blob, skipped"),
                }
            }
        }
        digests.sort_unstable();

        let mut out = Vec::new();
        for digest in digests.into_iter().filter(|d| after.is_none_or(|a| d > a)) {
            if out.len() == limit {
                break;
            }
            // Deleted since `read_dir` saw it.
            let Some(meta) = none_if_missing(fs::metadata(self.path(&digest)).await)? else {
                continue;
            };
            out.push(BlobEntry {
                digest,
                size: meta.len(),
                modified: UnixTime::from_system(meta.modified()?),
            });
        }
        Ok(out)
    }

    async fn put(&self, digest: &Digest, file: StagedFile) -> Result<(), StoreError> {
        let path = self.path(digest);

        // Two puts of one digest may both pass this check; the second rename then
        // replaces the file with the same bytes (the digest was checked), and a
        // reader holding the old file keeps reading it.
        if fs::try_exists(&path).await? {
            return Ok(());
        }
        file.rename_to(&path).await?;
        sync_parent(&path).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::FsBlobStore;
    use crate::store::conformance::{Blobs, TempDir, fixture, sha256};
    use crate::store::{BlobStore, Staging};
    use tokio::io::AsyncReadExt;

    async fn blobs() -> Blobs<FsBlobStore> {
        let dir = TempDir::new();
        let store = FsBlobStore::open(dir.path()).await.unwrap();
        let staging = Staging::open(dir.path().join("uploads")).await.unwrap();
        Blobs::in_dir(store, staging, dir)
    }

    crate::store::conformance::blob_store_suite!(super::blobs().await);

    #[tokio::test]
    async fn blobs_survive_reopen() {
        let t = blobs().await;
        let data = fixture(1000, 1);
        let digest = sha256(&data);
        t.staging.create(&"u1".parse().unwrap()).await.unwrap();
        t.staging
            .append(&"u1".parse().unwrap(), &data[..])
            .await
            .unwrap();
        let file = t.staging.finish(&"u1".parse().unwrap()).await.unwrap();
        t.store.put(&digest, file).await.unwrap();

        let root = t.staging.dir().parent().unwrap().to_owned();
        let reopened = FsBlobStore::open(&root).await.unwrap();
        let mut out = Vec::new();
        let mut body = reopened.get(&digest, None).await.unwrap().unwrap().body;
        body.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, data);
        assert_eq!(reopened.list(None, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn foreign_files_are_not_listed() {
        let t = blobs().await;
        let root = t.staging.dir().parent().unwrap().to_owned();
        std::fs::write(root.join("blobs/sha256/notes.txt"), b"x").unwrap();

        assert!(t.store.list(None, 10).await.unwrap().is_empty());
    }
}
