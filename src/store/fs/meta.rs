use super::{remove_tmp_files, replace, sync_parent, write_atomic};
use crate::oci::Digest;
use crate::store::StoreError;
use crate::store::index::{Index, IndexStorage, IndexedMeta};
use crate::store::staging::remove_if_exists;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Bumped when the index format changes; an index of another version does not load.
const INDEX_VERSION: u32 = 1;

/// Meta on the local filesystem: the index in memory, rewritten in full to
/// `meta/index.json` on every change, manifest bodies as separate files.
pub type FsMeta = IndexedMeta<FsMetaFiles>;

pub struct FsMetaFiles {
    index: PathBuf,
    manifests: PathBuf,
}

#[derive(Serialize)]
struct IndexFileRef<'a> {
    version: u32,
    index: &'a Index,
}

#[derive(Deserialize)]
struct IndexFile {
    version: u32,
    index: Index,
}

impl FsMeta {
    /// `root` is the storage root; meta goes to `root/meta`. A missing index is an
    /// empty registry; an unreadable one is an error, never a silent reset.
    pub async fn open(root: &Path) -> Result<Self, StoreError> {
        let dir = root.join("meta");
        let manifests = dir.join("manifests");
        for alg in ["sha256", "sha512"] {
            fs::create_dir_all(manifests.join(alg)).await?;
            remove_tmp_files(&manifests.join(alg)).await?;
        }
        remove_tmp_files(&dir).await?;

        let files = FsMetaFiles {
            index: dir.join("index.json"),
            manifests,
        };
        let index = files.load().await?;
        Ok(Self::new(index, files))
    }
}

impl FsMetaFiles {
    async fn load(&self) -> Result<Index, StoreError> {
        let data = match fs::read(&self.index).await {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Index::default()),
            Err(e) => return Err(e.into()),
        };

        let corrupt = |reason: String| StoreError::Corrupt {
            path: self.index.clone(),
            reason,
        };
        let file: IndexFile = serde_json::from_slice(&data).map_err(|e| corrupt(e.to_string()))?;
        if file.version != INDEX_VERSION {
            return Err(corrupt(format!(
                "index version {} is not supported, expected {INDEX_VERSION}",
                file.version
            )));
        }
        Ok(file.index)
    }

    fn body_path(&self, digest: &Digest) -> PathBuf {
        self.manifests.join(digest.algorithm()).join(digest.hex())
    }
}

impl IndexStorage for FsMetaFiles {
    async fn save(&self, index: &Index) -> Result<(), StoreError> {
        let data = serde_json::to_vec(&IndexFileRef {
            version: INDEX_VERSION,
            index,
        })
        .map_err(io::Error::other)?;
        replace(&self.index, &data).await?;

        // The new index is in place and is what a restart reads, so memory has to
        // move to it: an error here would keep the old index in memory and the next
        // write would put it back on disk, dropping this change. A failing directory
        // fsync means a failing disk, which the next write reports.
        if let Err(err) = sync_parent(&self.index).await {
            tracing::error!(path = %self.index.display(), %err, "cannot sync meta directory");
        }
        Ok(())
    }

    async fn put_body(&self, digest: &Digest, body: &Bytes) -> Result<(), StoreError> {
        let path = self.body_path(digest);
        if fs::try_exists(&path).await? {
            return Ok(());
        }
        Ok(write_atomic(&path, body).await?)
    }

    async fn get_body(&self, digest: &Digest) -> Result<Option<Bytes>, StoreError> {
        match fs::read(self.body_path(digest)).await {
            Ok(data) => Ok(Some(Bytes::from(data))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn delete_body(&self, digest: &Digest) -> Result<(), StoreError> {
        let path = self.body_path(digest);
        if remove_if_exists(&path).await? {
            sync_parent(&path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::FsMeta;
    use crate::oci::{Reference, RepoName, Tag};
    use crate::store::conformance::{Held, TempDir, fixture, sha256};
    use crate::store::{
        AuthMeta, NewManifest, RegistryMeta, StoreError, TokenHash, TokenRecord, UnixTime, User,
    };
    use bytes::Bytes;

    async fn fixture_meta() -> Held<FsMeta> {
        let dir = TempDir::new();
        Held::in_dir(FsMeta::open(dir.path()).await.unwrap(), dir)
    }

    crate::store::conformance::meta_suite!(super::fixture_meta().await);

    fn manifest(seed: u64) -> NewManifest {
        let body = Bytes::from(fixture(300, seed));
        NewManifest {
            digest: sha256(&body),
            media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            body,
            blobs: Vec::new(),
            children: Vec::new(),
            subject: None,
            artifact_type: None,
        }
    }

    #[tokio::test]
    async fn state_survives_reopen() {
        let dir = TempDir::new();
        let repo: RepoName = "team/app".parse().unwrap();
        let tag: Tag = "v1".parse().unwrap();
        let m = manifest(1);
        let alice = User {
            name: "alice".parse().unwrap(),
            admin: true,
            created: UnixTime(1_700_000_000),
        };

        let meta = FsMeta::open(dir.path()).await.unwrap();
        meta.put_manifest(&repo, &m, Some(&tag)).await.unwrap();
        meta.set_repo_public(&repo, true).await.unwrap();
        meta.create_user(&alice).await.unwrap();
        let token = TokenRecord {
            id: "aaa1".parse().unwrap(),
            hash: TokenHash([0xab; 32]),
            user: alice.name.clone(),
            description: "ci".into(),
            scopes: vec!["repository:team/*:pull".into()],
            expires: None,
            created: UnixTime(1_700_000_000),
            last_used: Some(UnixTime(1_750_000_000)),
        };
        meta.create_token(&token).await.unwrap();
        let before = meta.get_repo(&repo).await.unwrap();
        drop(meta);

        let meta = FsMeta::open(dir.path()).await.unwrap();
        assert_eq!(meta.get_repo(&repo).await.unwrap(), before);
        let got = meta
            .get_manifest(&repo, &Reference::Tag(tag))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.body, m.body);
        assert_eq!(meta.list_users().await.unwrap(), [alice]);
        assert_eq!(
            meta.get_token_by_hash(&token.hash).await.unwrap(),
            Some(token)
        );
    }

    #[tokio::test]
    async fn body_shared_by_repos_outlives_one_delete() {
        let dir = TempDir::new();
        let f = FsMeta::open(dir.path()).await.unwrap();
        let (a, b): (RepoName, RepoName) = ("a".parse().unwrap(), "b".parse().unwrap());
        let m = manifest(2);
        f.put_manifest(&a, &m, None).await.unwrap();
        f.put_manifest(&b, &m, None).await.unwrap();

        f.delete_repo(&a).await.unwrap();
        assert!(
            f.get_manifest(&b, &Reference::Digest(m.digest))
                .await
                .unwrap()
                .is_some()
        );

        f.delete_manifest(&b, &m.digest).await.unwrap();
        let body = dir
            .path()
            .join("meta/manifests/sha256")
            .join(m.digest.hex());
        assert!(!body.exists(), "the last reference takes the body with it");
    }

    #[tokio::test]
    async fn corrupt_index_fails_to_open() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("meta")).unwrap();
        std::fs::write(dir.path().join("meta/index.json"), b"{\"version\":1,\"ind").unwrap();

        assert!(matches!(
            FsMeta::open(dir.path()).await,
            Err(StoreError::Corrupt { .. })
        ));
    }

    #[tokio::test]
    async fn unknown_index_version_fails_to_open() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("meta")).unwrap();
        std::fs::write(
            dir.path().join("meta/index.json"),
            br#"{"version":2,"index":{"repos":{},"users":{},"tokens":{}}}"#,
        )
        .unwrap();

        assert!(matches!(
            FsMeta::open(dir.path()).await,
            Err(StoreError::Corrupt { .. })
        ));
    }

    #[tokio::test]
    async fn invalid_value_in_index_fails_to_open() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("meta")).unwrap();
        std::fs::write(
            dir.path().join("meta/index.json"),
            br#"{"version":1,"index":{"repos":{"Not/Valid":{"public":false,"updated":0,"manifests":{},"tags":{}}},"users":{},"tokens":{}}}"#,
        )
        .unwrap();

        assert!(matches!(
            FsMeta::open(dir.path()).await,
            Err(StoreError::Corrupt { .. })
        ));
    }

    /// A crash mid-write leaves `index.json.tmp`: the old index loads, the leftover goes.
    #[tokio::test]
    async fn leftover_tmp_index_is_ignored() {
        let dir = TempDir::new();
        let repo: RepoName = "app".parse().unwrap();
        let meta = FsMeta::open(dir.path()).await.unwrap();
        meta.put_manifest(&repo, &manifest(3), None).await.unwrap();
        drop(meta);
        let tmp = dir.path().join("meta/index.json.tmp");
        std::fs::write(&tmp, b"half a json").unwrap();

        let meta = FsMeta::open(dir.path()).await.unwrap();
        assert!(meta.get_repo(&repo).await.unwrap().is_some());
        assert!(!tmp.exists());
    }
}
