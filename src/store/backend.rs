//! Backends chosen by config, dispatched through enums, and opening them at startup.

use super::fs::{FsBlobStore, FsMeta};
use super::{
    AuthMeta, BlobEntry, BlobRead, BlobStore, ByteRange, Manifest, ManifestSummary, NewManifest,
    RegistryMeta, RepoSummary, StagedFile, Staging, StoreError, TagEntry, TokenHash, TokenId,
    TokenRecord, UnixTime, User, UserName,
};
use crate::config::{MetaConfig, StorageConfig, UploadsConfig};
use crate::oci::{Digest, Reference, RepoName, Tag};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub enum BlobBackend {
    Fs(FsBlobStore),
}

pub enum MetaBackend {
    Fs(FsMeta),
}

/// Everything the server stores, opened from config.
pub struct Storage {
    pub blobs: BlobBackend,
    pub meta: MetaBackend,
    pub staging: Staging,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(
        "uploads.dir {} is on another filesystem than storage.root {}: finished uploads are moved into the store with a rename",
        uploads.display(),
        root.display()
    )]
    CrossDevice { uploads: PathBuf, root: PathBuf },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Creates what is missing on disk and loads the meta. Fails on a damaged meta
/// index instead of starting with an empty registry.
pub async fn open(
    storage: &StorageConfig,
    meta: &MetaConfig,
    uploads: &UploadsConfig,
) -> Result<Storage, OpenError> {
    match (storage, meta) {
        (StorageConfig::Fs { root }, MetaConfig::Storage {}) => {
            let blobs = FsBlobStore::open(root).await?;
            let meta = FsMeta::open(root).await?;
            let dir = uploads.dir.clone().unwrap_or_else(|| root.join("uploads"));
            let staging = Staging::open(dir).await?;
            check_same_device(staging.dir(), root).await?;

            Ok(Storage {
                blobs: BlobBackend::Fs(blobs),
                meta: MetaBackend::Fs(meta),
                staging,
            })
        }
    }
}

#[cfg(unix)]
async fn check_same_device(uploads: &Path, root: &Path) -> Result<(), OpenError> {
    use std::os::unix::fs::MetadataExt;

    let dev = |p: &Path| {
        let p = p.to_owned();
        async move { tokio::fs::metadata(&p).await.map(|m| m.dev()) }
    };
    if dev(uploads).await.map_err(StoreError::from)? != dev(root).await.map_err(StoreError::from)? {
        return Err(OpenError::CrossDevice {
            uploads: uploads.to_owned(),
            root: root.to_owned(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
async fn check_same_device(_uploads: &Path, _root: &Path) -> Result<(), OpenError> {
    // The rename in `put` reports a cross-device move itself.
    Ok(())
}

macro_rules! dispatch {
    ($self:ident, $backend:ident => $call:expr) => {
        match $self {
            Self::Fs($backend) => $call,
        }
    };
}

impl BlobStore for BlobBackend {
    async fn stat(&self, digest: &Digest) -> Result<Option<u64>, StoreError> {
        dispatch!(self, b => b.stat(digest).await)
    }

    async fn get(
        &self,
        digest: &Digest,
        range: Option<ByteRange>,
    ) -> Result<Option<BlobRead>, StoreError> {
        dispatch!(self, b => b.get(digest, range).await)
    }

    async fn presign_get(
        &self,
        digest: &Digest,
        ttl: Duration,
    ) -> Result<Option<String>, StoreError> {
        dispatch!(self, b => b.presign_get(digest, ttl).await)
    }

    async fn delete(&self, digest: &Digest) -> Result<bool, StoreError> {
        dispatch!(self, b => b.delete(digest).await)
    }

    async fn list(
        &self,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Vec<BlobEntry>, StoreError> {
        dispatch!(self, b => b.list(after, limit).await)
    }

    async fn put(&self, digest: &Digest, file: StagedFile) -> Result<(), StoreError> {
        dispatch!(self, b => b.put(digest, file).await)
    }
}

impl RegistryMeta for MetaBackend {
    async fn list_repos(
        &self,
        after: Option<&RepoName>,
        limit: usize,
    ) -> Result<Vec<RepoSummary>, StoreError> {
        dispatch!(self, m => m.list_repos(after, limit).await)
    }

    async fn get_repo(&self, repo: &RepoName) -> Result<Option<RepoSummary>, StoreError> {
        dispatch!(self, m => m.get_repo(repo).await)
    }

    async fn set_repo_public(&self, repo: &RepoName, public: bool) -> Result<bool, StoreError> {
        dispatch!(self, m => m.set_repo_public(repo, public).await)
    }

    async fn delete_repo(&self, repo: &RepoName) -> Result<bool, StoreError> {
        dispatch!(self, m => m.delete_repo(repo).await)
    }

    async fn get_manifest(
        &self,
        repo: &RepoName,
        reference: &Reference,
    ) -> Result<Option<Manifest>, StoreError> {
        dispatch!(self, m => m.get_manifest(repo, reference).await)
    }

    async fn put_manifest(
        &self,
        repo: &RepoName,
        manifest: &NewManifest,
        tag: Option<&Tag>,
    ) -> Result<(), StoreError> {
        dispatch!(self, m => m.put_manifest(repo, manifest, tag).await)
    }

    async fn delete_manifest(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        dispatch!(self, m => m.delete_manifest(repo, digest).await)
    }

    async fn list_manifests(
        &self,
        repo: &RepoName,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Option<Vec<ManifestSummary>>, StoreError> {
        dispatch!(self, m => m.list_manifests(repo, after, limit).await)
    }

    async fn list_tags(
        &self,
        repo: &RepoName,
        after: Option<&Tag>,
        limit: usize,
    ) -> Result<Option<Vec<TagEntry>>, StoreError> {
        dispatch!(self, m => m.list_tags(repo, after, limit).await)
    }

    async fn delete_tag(&self, repo: &RepoName, tag: &Tag) -> Result<bool, StoreError> {
        dispatch!(self, m => m.delete_tag(repo, tag).await)
    }

    async fn list_referrers(
        &self,
        repo: &RepoName,
        subject: &Digest,
    ) -> Result<Vec<ManifestSummary>, StoreError> {
        dispatch!(self, m => m.list_referrers(repo, subject).await)
    }

    async fn repo_has_blob(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        dispatch!(self, m => m.repo_has_blob(repo, digest).await)
    }

    async fn referenced_blobs(&self) -> Result<Vec<Digest>, StoreError> {
        dispatch!(self, m => m.referenced_blobs().await)
    }
}

impl AuthMeta for MetaBackend {
    async fn get_user(&self, name: &UserName) -> Result<Option<User>, StoreError> {
        dispatch!(self, m => m.get_user(name).await)
    }

    async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        dispatch!(self, m => m.list_users().await)
    }

    async fn create_user(&self, user: &User) -> Result<(), StoreError> {
        dispatch!(self, m => m.create_user(user).await)
    }

    async fn update_user(&self, user: &User) -> Result<bool, StoreError> {
        dispatch!(self, m => m.update_user(user).await)
    }

    async fn delete_user(&self, name: &UserName) -> Result<bool, StoreError> {
        dispatch!(self, m => m.delete_user(name).await)
    }

    async fn create_token(&self, token: &TokenRecord) -> Result<(), StoreError> {
        dispatch!(self, m => m.create_token(token).await)
    }

    async fn get_token_by_hash(&self, hash: &TokenHash) -> Result<Option<TokenRecord>, StoreError> {
        dispatch!(self, m => m.get_token_by_hash(hash).await)
    }

    async fn list_tokens(&self, user: Option<&UserName>) -> Result<Vec<TokenRecord>, StoreError> {
        dispatch!(self, m => m.list_tokens(user).await)
    }

    async fn revoke_token(&self, id: &TokenId) -> Result<bool, StoreError> {
        dispatch!(self, m => m.revoke_token(id).await)
    }

    async fn set_token_last_used(&self, id: &TokenId, at: UnixTime) -> Result<bool, StoreError> {
        dispatch!(self, m => m.set_token_last_used(id, at).await)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::conformance::TempDir;

    fn fs(root: &Path) -> StorageConfig {
        StorageConfig::Fs {
            root: root.to_owned(),
        }
    }

    #[tokio::test]
    async fn fs_creates_layout_with_uploads_under_root() {
        let dir = TempDir::new();
        let root = dir.path().join("data");

        let storage = open(
            &fs(&root),
            &MetaConfig::Storage {},
            &UploadsConfig { dir: None },
        )
        .await
        .unwrap();

        assert_eq!(storage.staging.dir(), root.join("uploads"));
        for sub in [
            "blobs/sha256",
            "blobs/sha512",
            "meta/manifests/sha256",
            "uploads",
        ] {
            assert!(root.join(sub).is_dir(), "{sub} missing");
        }
        assert!(storage.blobs.list(None, 1).await.unwrap().is_empty());
        assert!(storage.meta.list_repos(None, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fs_takes_uploads_dir_from_config() {
        let dir = TempDir::new();
        let root = dir.path().join("data");
        let uploads = dir.path().join("scratch");

        let storage = open(
            &fs(&root),
            &MetaConfig::Storage {},
            &UploadsConfig {
                dir: Some(uploads.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(storage.staging.dir(), uploads);
    }

    #[tokio::test]
    async fn fs_refuses_damaged_meta() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("meta")).unwrap();
        std::fs::write(dir.path().join("meta/index.json"), b"not json").unwrap();

        let result = open(
            &fs(dir.path()),
            &MetaConfig::Storage {},
            &UploadsConfig { dir: None },
        )
        .await;
        assert!(matches!(
            result,
            Err(OpenError::Store(StoreError::Corrupt { .. }))
        ));
    }

    /// `/dev/shm` is a tmpfs on Linux, so it is another device than the temp dir
    /// unless the temp dir is a tmpfs too.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn fs_refuses_uploads_on_another_filesystem() {
        use std::os::unix::fs::MetadataExt;

        let dir = TempDir::new();
        let shm = Path::new("/dev/shm");
        let (Ok(a), Ok(b)) = (std::fs::metadata(dir.path()), std::fs::metadata(shm)) else {
            return;
        };
        if a.dev() == b.dev() {
            return;
        }
        let uploads = shm.join(format!("tiny-repo-test-{}", std::process::id()));

        let result = open(
            &fs(dir.path()),
            &MetaConfig::Storage {},
            &UploadsConfig {
                dir: Some(uploads.clone()),
            },
        )
        .await;
        let _ = std::fs::remove_dir_all(&uploads);
        assert!(matches!(result, Err(OpenError::CrossDevice { .. })));
    }
}
