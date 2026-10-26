//! In-memory backend for tests: the reference the conformance suite is checked
//! against, and a fast store for tests of the layers above.

use super::{
    AuthMeta, BlobEntry, BlobRead, BlobStore, ByteRange, Manifest, ManifestSummary, NewManifest,
    RegistryMeta, RepoSummary, StoreError, TagEntry, TokenHash, TokenId, TokenRecord, UnixTime,
    UploadEntry, User, UserName,
};
use crate::oci::{Digest, Reference, RepoName, Tag, UploadId};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Cursor;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct MemoryBlobStore {
    state: Mutex<BlobState>,
}

#[derive(Default)]
struct BlobState {
    blobs: BTreeMap<Digest, (Bytes, UnixTime)>,
    uploads: HashMap<UploadId, (Vec<u8>, UnixTime)>,
}

impl BlobStore for MemoryBlobStore {
    async fn stat(&self, digest: &Digest) -> Result<Option<u64>, StoreError> {
        let state = self.state.lock().await;
        Ok(state.blobs.get(digest).map(|(data, _)| data.len() as u64))
    }

    async fn get(
        &self,
        digest: &Digest,
        range: Option<ByteRange>,
    ) -> Result<Option<BlobRead>, StoreError> {
        let Some(data) = self
            .state
            .lock()
            .await
            .blobs
            .get(digest)
            .map(|(d, _)| d.clone())
        else {
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
        Ok(self.state.lock().await.blobs.remove(digest).is_some())
    }

    async fn list(
        &self,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Vec<BlobEntry>, StoreError> {
        let state = self.state.lock().await;
        Ok(state
            .blobs
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

    async fn start_upload(&self, id: &UploadId) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        if state.uploads.contains_key(id) {
            return Err(StoreError::AlreadyExists);
        }
        state
            .uploads
            .insert(id.clone(), (Vec::new(), UnixTime::now()));
        Ok(())
    }

    async fn append(
        &self,
        id: &UploadId,
        mut body: impl AsyncRead + Send + Unpin,
    ) -> Result<u64, StoreError> {
        if !self.state.lock().await.uploads.contains_key(id) {
            return Err(StoreError::UploadUnknown);
        }

        // Read without holding the lock, then check again: the upload may have been
        // aborted meanwhile.
        let mut chunk = Vec::new();
        body.read_to_end(&mut chunk).await?;

        let mut state = self.state.lock().await;
        let (data, _) = state.uploads.get_mut(id).ok_or(StoreError::UploadUnknown)?;
        data.extend_from_slice(&chunk);
        Ok(chunk.len() as u64)
    }

    async fn commit(&self, id: &UploadId, digest: &Digest) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        let (data, _) = state.uploads.remove(id).ok_or(StoreError::UploadUnknown)?;
        state
            .blobs
            .entry(*digest)
            .or_insert_with(|| (Bytes::from(data), UnixTime::now()));
        Ok(())
    }

    async fn abort(&self, id: &UploadId) -> Result<bool, StoreError> {
        Ok(self.state.lock().await.uploads.remove(id).is_some())
    }

    async fn list_uploads(&self) -> Result<Vec<UploadEntry>, StoreError> {
        let state = self.state.lock().await;
        Ok(state
            .uploads
            .iter()
            .map(|(id, (_, started))| UploadEntry {
                id: id.clone(),
                started: *started,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct MemoryMeta {
    state: Mutex<MetaState>,
}

#[derive(Default)]
struct MetaState {
    repos: BTreeMap<RepoName, Repo>,
    users: BTreeMap<UserName, User>,
    tokens: BTreeMap<TokenId, TokenRecord>,
}

struct Repo {
    public: bool,
    updated: UnixTime,
    manifests: BTreeMap<Digest, (NewManifest, UnixTime)>,
    tags: BTreeMap<Tag, (Digest, UnixTime)>,
}

impl Repo {
    fn summary(&self, name: &RepoName) -> RepoSummary {
        let blobs: BTreeMap<Digest, u64> = self
            .manifests
            .values()
            .flat_map(|(m, _)| &m.blobs)
            .map(|b| (b.digest, b.size))
            .collect();

        RepoSummary {
            name: name.clone(),
            public: self.public,
            tag_count: self.tags.len(),
            manifest_count: self.manifests.len(),
            size: blobs.values().sum(),
            updated: self.updated,
        }
    }

    fn manifest_summary(&self, manifest: &NewManifest, created: UnixTime) -> ManifestSummary {
        ManifestSummary {
            digest: manifest.digest,
            media_type: manifest.media_type.clone(),
            artifact_type: manifest.artifact_type.clone(),
            size: manifest.body.len() as u64,
            tags: self
                .tags
                .iter()
                .filter(|(_, (d, _))| *d == manifest.digest)
                .map(|(t, _)| t.clone())
                .collect(),
            created,
        }
    }
}

impl RegistryMeta for MemoryMeta {
    async fn list_repos(
        &self,
        after: Option<&RepoName>,
        limit: usize,
    ) -> Result<Vec<RepoSummary>, StoreError> {
        let state = self.state.lock().await;
        Ok(state
            .repos
            .iter()
            .filter(|(name, _)| after.is_none_or(|a| *name > a))
            .take(limit)
            .map(|(name, repo)| repo.summary(name))
            .collect())
    }

    async fn get_repo(&self, repo: &RepoName) -> Result<Option<RepoSummary>, StoreError> {
        let state = self.state.lock().await;
        Ok(state.repos.get(repo).map(|r| r.summary(repo)))
    }

    async fn set_repo_public(&self, repo: &RepoName, public: bool) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(r) = state.repos.get_mut(repo) else {
            return Ok(false);
        };
        r.public = public;
        r.updated = UnixTime::now();
        Ok(true)
    }

    async fn delete_repo(&self, repo: &RepoName) -> Result<bool, StoreError> {
        Ok(self.state.lock().await.repos.remove(repo).is_some())
    }

    async fn get_manifest(
        &self,
        repo: &RepoName,
        reference: &Reference,
    ) -> Result<Option<Manifest>, StoreError> {
        let state = self.state.lock().await;
        let Some(r) = state.repos.get(repo) else {
            return Ok(None);
        };

        let digest = match reference {
            Reference::Digest(d) => Some(d),
            Reference::Tag(t) => r.tags.get(t).map(|(d, _)| d),
        };

        Ok(digest
            .and_then(|d| r.manifests.get(d))
            .map(|(m, _)| Manifest {
                digest: m.digest,
                media_type: m.media_type.clone(),
                body: m.body.clone(),
            }))
    }

    async fn put_manifest(
        &self,
        repo: &RepoName,
        manifest: &NewManifest,
        tag: Option<&Tag>,
    ) -> Result<(), StoreError> {
        let now = UnixTime::now();
        let mut state = self.state.lock().await;

        let r = state.repos.entry(repo.clone()).or_insert_with(|| Repo {
            public: false,
            updated: now,
            manifests: BTreeMap::new(),
            tags: BTreeMap::new(),
        });

        r.manifests
            .entry(manifest.digest)
            .or_insert_with(|| (manifest.clone(), now));
        if let Some(tag) = tag {
            r.tags.insert(tag.clone(), (manifest.digest, now));
        }
        r.updated = now;
        Ok(())
    }

    async fn delete_manifest(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(r) = state.repos.get_mut(repo) else {
            return Ok(false);
        };
        if r.manifests.remove(digest).is_none() {
            return Ok(false);
        }
        r.tags.retain(|_, (d, _)| d != digest);
        r.updated = UnixTime::now();
        Ok(true)
    }

    async fn list_manifests(
        &self,
        repo: &RepoName,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Option<Vec<ManifestSummary>>, StoreError> {
        let state = self.state.lock().await;
        Ok(state.repos.get(repo).map(|r| {
            r.manifests
                .iter()
                .filter(|(d, _)| after.is_none_or(|a| *d > a))
                .take(limit)
                .map(|(_, (m, created))| r.manifest_summary(m, *created))
                .collect()
        }))
    }

    async fn list_tags(
        &self,
        repo: &RepoName,
        after: Option<&Tag>,
        limit: usize,
    ) -> Result<Option<Vec<TagEntry>>, StoreError> {
        let state = self.state.lock().await;
        Ok(state.repos.get(repo).map(|r| {
            r.tags
                .iter()
                .filter(|(t, _)| after.is_none_or(|a| *t > a))
                .take(limit)
                .map(|(t, (d, updated))| TagEntry {
                    tag: t.clone(),
                    digest: *d,
                    updated: *updated,
                })
                .collect()
        }))
    }

    async fn delete_tag(&self, repo: &RepoName, tag: &Tag) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(r) = state.repos.get_mut(repo) else {
            return Ok(false);
        };
        if r.tags.remove(tag).is_none() {
            return Ok(false);
        }
        r.updated = UnixTime::now();
        Ok(true)
    }

    async fn list_referrers(
        &self,
        repo: &RepoName,
        subject: &Digest,
    ) -> Result<Vec<ManifestSummary>, StoreError> {
        let state = self.state.lock().await;
        let Some(r) = state.repos.get(repo) else {
            return Ok(Vec::new());
        };
        Ok(r.manifests
            .values()
            .filter(|(m, _)| m.subject.as_ref() == Some(subject))
            .map(|(m, created)| r.manifest_summary(m, *created))
            .collect())
    }

    async fn repo_has_blob(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        let state = self.state.lock().await;
        Ok(state.repos.get(repo).is_some_and(|r| {
            r.manifests
                .values()
                .any(|(m, _)| m.blobs.iter().any(|b| b.digest == *digest))
        }))
    }

    async fn referenced_blobs(&self) -> Result<Vec<Digest>, StoreError> {
        let state = self.state.lock().await;
        let all: BTreeSet<Digest> = state
            .repos
            .values()
            .flat_map(|r| r.manifests.values())
            .flat_map(|(m, _)| m.blobs.iter().map(|b| b.digest))
            .collect();
        Ok(all.into_iter().collect())
    }
}

impl AuthMeta for MemoryMeta {
    async fn get_user(&self, name: &UserName) -> Result<Option<User>, StoreError> {
        Ok(self.state.lock().await.users.get(name).cloned())
    }

    async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        Ok(self.state.lock().await.users.values().cloned().collect())
    }

    async fn create_user(&self, user: &User) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        if state.users.contains_key(&user.name) {
            return Err(StoreError::AlreadyExists);
        }
        state.users.insert(user.name.clone(), user.clone());
        Ok(())
    }

    async fn update_user(&self, user: &User) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(stored) = state.users.get_mut(&user.name) else {
            return Ok(false);
        };
        *stored = user.clone();
        Ok(true)
    }

    async fn delete_user(&self, name: &UserName) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        if state.users.remove(name).is_none() {
            return Ok(false);
        }
        state.tokens.retain(|_, t| t.user != *name);
        Ok(true)
    }

    async fn create_token(&self, token: &TokenRecord) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        if !state.users.contains_key(&token.user) {
            return Err(StoreError::UserUnknown);
        }
        if state.tokens.contains_key(&token.id)
            || state.tokens.values().any(|t| t.hash == token.hash)
        {
            return Err(StoreError::AlreadyExists);
        }
        state.tokens.insert(token.id.clone(), token.clone());
        Ok(())
    }

    async fn get_token_by_hash(&self, hash: &TokenHash) -> Result<Option<TokenRecord>, StoreError> {
        let state = self.state.lock().await;
        Ok(state.tokens.values().find(|t| t.hash == *hash).cloned())
    }

    async fn list_tokens(&self, user: Option<&UserName>) -> Result<Vec<TokenRecord>, StoreError> {
        let state = self.state.lock().await;
        Ok(state
            .tokens
            .values()
            .filter(|t| user.is_none_or(|u| t.user == *u))
            .cloned()
            .collect())
    }

    async fn revoke_token(&self, id: &TokenId) -> Result<bool, StoreError> {
        Ok(self.state.lock().await.tokens.remove(id).is_some())
    }

    async fn set_token_last_used(&self, id: &TokenId, at: UnixTime) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(t) = state.tokens.get_mut(id) else {
            return Ok(false);
        };
        t.last_used = Some(at);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryBlobStore, MemoryMeta};

    crate::store::conformance::blob_store_suite!(MemoryBlobStore::default());
    crate::store::conformance::meta_suite!(MemoryMeta::default());
}
