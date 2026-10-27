//! The whole meta of a single-writer backend as one in-memory value. `Index` holds
//! the records and the rules (what a put, retag or delete does); `IndexedMeta`
//! puts it behind one lock, implements the meta traits on it, and hands every new
//! state to an `IndexStorage` before making it visible. Manifest bodies stay out of
//! the index: they are up to 4 MB each and are only needed by `get_manifest`.

use super::{
    AuthMeta, BlobRef, Manifest, ManifestSummary, NewManifest, RegistryMeta, RepoSummary,
    StoreError, TagEntry, TokenHash, TokenId, TokenRecord, UnixTime, User, UserName,
};
use crate::oci::{Digest, Reference, RepoName, Tag};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    repos: BTreeMap<RepoName, Repo>,
    users: BTreeMap<UserName, User>,
    tokens: BTreeMap<TokenId, TokenRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Repo {
    public: bool,
    updated: UnixTime,
    manifests: BTreeMap<Digest, ManifestRecord>,
    tags: BTreeMap<Tag, TagRecord>,
}

/// `NewManifest` without its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestRecord {
    media_type: String,
    size: u64,
    blobs: Vec<BlobRef>,
    children: Vec<Digest>,
    subject: Option<Digest>,
    artifact_type: Option<String>,
    created: UnixTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TagRecord {
    digest: Digest,
    updated: UnixTime,
}

impl Repo {
    fn summary(&self, name: &RepoName) -> RepoSummary {
        let blobs: BTreeMap<Digest, u64> = self
            .manifests
            .values()
            .flat_map(|m| &m.blobs)
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

    fn manifest_summary(&self, digest: &Digest, m: &ManifestRecord) -> ManifestSummary {
        ManifestSummary {
            digest: *digest,
            media_type: m.media_type.clone(),
            artifact_type: m.artifact_type.clone(),
            size: m.size,
            tags: self
                .tags
                .iter()
                .filter(|(_, t)| t.digest == *digest)
                .map(|(tag, _)| tag.clone())
                .collect(),
            created: m.created,
        }
    }
}

/// Mutators return whether they changed anything, so a no-op is not persisted.
impl Index {
    fn list_repos(&self, after: Option<&RepoName>, limit: usize) -> Vec<RepoSummary> {
        self.repos
            .iter()
            .filter(|(name, _)| after.is_none_or(|a| *name > a))
            .take(limit)
            .map(|(name, repo)| repo.summary(name))
            .collect()
    }

    fn get_repo(&self, repo: &RepoName) -> Option<RepoSummary> {
        self.repos.get(repo).map(|r| r.summary(repo))
    }

    fn set_repo_public(&mut self, repo: &RepoName, public: bool) -> bool {
        let Some(r) = self.repos.get_mut(repo) else {
            return false;
        };
        r.public = public;
        r.updated = UnixTime::now();
        true
    }

    /// The manifests the repository had, `None` if it did not exist.
    fn delete_repo(&mut self, repo: &RepoName) -> Option<Vec<Digest>> {
        self.repos
            .remove(repo)
            .map(|r| r.manifests.into_keys().collect())
    }

    /// Digest and media type of the manifest a reference points at.
    fn resolve(&self, repo: &RepoName, reference: &Reference) -> Option<(Digest, String)> {
        let r = self.repos.get(repo)?;
        let digest = match reference {
            Reference::Digest(d) => d,
            Reference::Tag(t) => &r.tags.get(t)?.digest,
        };
        r.manifests
            .get(digest)
            .map(|m| (*digest, m.media_type.clone()))
    }

    fn put_manifest(&mut self, repo: &RepoName, manifest: &NewManifest, tag: Option<&Tag>) {
        let now = UnixTime::now();
        let r = self.repos.entry(repo.clone()).or_insert_with(|| Repo {
            public: false,
            updated: now,
            manifests: BTreeMap::new(),
            tags: BTreeMap::new(),
        });

        r.manifests
            .entry(manifest.digest)
            .or_insert_with(|| ManifestRecord {
                media_type: manifest.media_type.clone(),
                size: manifest.body.len() as u64,
                blobs: manifest.blobs.clone(),
                children: manifest.children.clone(),
                subject: manifest.subject,
                artifact_type: manifest.artifact_type.clone(),
                created: now,
            });
        if let Some(tag) = tag {
            r.tags.insert(
                tag.clone(),
                TagRecord {
                    digest: manifest.digest,
                    updated: now,
                },
            );
        }
        r.updated = now;
    }

    fn delete_manifest(&mut self, repo: &RepoName, digest: &Digest) -> bool {
        let Some(r) = self.repos.get_mut(repo) else {
            return false;
        };
        if r.manifests.remove(digest).is_none() {
            return false;
        }
        r.tags.retain(|_, t| t.digest != *digest);
        r.updated = UnixTime::now();
        true
    }

    /// Whether any repository still has the manifest, i.e. its body is still needed.
    fn manifest_in_use(&self, digest: &Digest) -> bool {
        self.repos
            .values()
            .any(|r| r.manifests.contains_key(digest))
    }

    fn list_manifests(
        &self,
        repo: &RepoName,
        after: Option<&Digest>,
        limit: usize,
    ) -> Option<Vec<ManifestSummary>> {
        self.repos.get(repo).map(|r| {
            r.manifests
                .iter()
                .filter(|(d, _)| after.is_none_or(|a| *d > a))
                .take(limit)
                .map(|(d, m)| r.manifest_summary(d, m))
                .collect()
        })
    }

    fn list_tags(
        &self,
        repo: &RepoName,
        after: Option<&Tag>,
        limit: usize,
    ) -> Option<Vec<TagEntry>> {
        self.repos.get(repo).map(|r| {
            r.tags
                .iter()
                .filter(|(t, _)| after.is_none_or(|a| *t > a))
                .take(limit)
                .map(|(t, rec)| TagEntry {
                    tag: t.clone(),
                    digest: rec.digest,
                    updated: rec.updated,
                })
                .collect()
        })
    }

    fn delete_tag(&mut self, repo: &RepoName, tag: &Tag) -> bool {
        let Some(r) = self.repos.get_mut(repo) else {
            return false;
        };
        if r.tags.remove(tag).is_none() {
            return false;
        }
        r.updated = UnixTime::now();
        true
    }

    fn list_referrers(&self, repo: &RepoName, subject: &Digest) -> Vec<ManifestSummary> {
        let Some(r) = self.repos.get(repo) else {
            return Vec::new();
        };
        r.manifests
            .iter()
            .filter(|(_, m)| m.subject.as_ref() == Some(subject))
            .map(|(d, m)| r.manifest_summary(d, m))
            .collect()
    }

    fn repo_has_blob(&self, repo: &RepoName, digest: &Digest) -> bool {
        self.repos.get(repo).is_some_and(|r| {
            r.manifests
                .values()
                .any(|m| m.blobs.iter().any(|b| b.digest == *digest))
        })
    }

    fn referenced_blobs(&self) -> Vec<Digest> {
        let all: BTreeSet<Digest> = self
            .repos
            .values()
            .flat_map(|r| r.manifests.values())
            .flat_map(|m| m.blobs.iter().map(|b| b.digest))
            .collect();
        all.into_iter().collect()
    }

    fn create_user(&mut self, user: &User) -> Result<(), StoreError> {
        if self.users.contains_key(&user.name) {
            return Err(StoreError::AlreadyExists);
        }
        self.users.insert(user.name.clone(), user.clone());
        Ok(())
    }

    fn update_user(&mut self, user: &User) -> bool {
        let Some(stored) = self.users.get_mut(&user.name) else {
            return false;
        };
        *stored = user.clone();
        true
    }

    fn delete_user(&mut self, name: &UserName) -> bool {
        if self.users.remove(name).is_none() {
            return false;
        }
        self.tokens.retain(|_, t| t.user != *name);
        true
    }

    fn create_token(&mut self, token: &TokenRecord) -> Result<(), StoreError> {
        if !self.users.contains_key(&token.user) {
            return Err(StoreError::UserUnknown);
        }
        if self.tokens.contains_key(&token.id) || self.tokens.values().any(|t| t.hash == token.hash)
        {
            return Err(StoreError::AlreadyExists);
        }
        self.tokens.insert(token.id.clone(), token.clone());
        Ok(())
    }

    fn get_token_by_hash(&self, hash: &TokenHash) -> Option<TokenRecord> {
        self.tokens.values().find(|t| t.hash == *hash).cloned()
    }

    fn list_tokens(&self, user: Option<&UserName>) -> Vec<TokenRecord> {
        self.tokens
            .values()
            .filter(|t| user.is_none_or(|u| t.user == *u))
            .cloned()
            .collect()
    }

    fn set_token_last_used(&mut self, id: &TokenId, at: UnixTime) -> bool {
        let Some(t) = self.tokens.get_mut(id) else {
            return false;
        };
        t.last_used = Some(at);
        true
    }
}

/// Where an `IndexedMeta` keeps its state. `save` gets every new index before it
/// becomes visible, so an error leaves the old state in place.
pub trait IndexStorage {
    /// An error must mean the stored index is still the previous one; once the new
    /// one is stored, `save` succeeds.
    async fn save(&self, index: &Index) -> Result<(), StoreError>;

    /// Bodies are addressed by digest and shared by repositories; storing one that
    /// exists is a no-op.
    async fn put_body(&self, digest: &Digest, body: &Bytes) -> Result<(), StoreError>;

    async fn get_body(&self, digest: &Digest) -> Result<Option<Bytes>, StoreError>;

    async fn delete_body(&self, digest: &Digest) -> Result<(), StoreError>;
}

/// Writes take the lock exclusively and keep it while they persist, which is the
/// single-writer model: one process, writes applied and stored in one order.
pub struct IndexedMeta<S> {
    index: RwLock<Index>,
    storage: S,
}

impl<S: Default> Default for IndexedMeta<S> {
    fn default() -> Self {
        Self::new(Index::default(), S::default())
    }
}

impl<S> IndexedMeta<S> {
    pub fn new(index: Index, storage: S) -> Self {
        Self {
            index: RwLock::new(index),
            storage,
        }
    }
}

impl<S: IndexStorage> IndexedMeta<S> {
    /// Applies `change` to a copy and swaps it in once stored. The copy is the price
    /// of not leaving memory ahead of disk after a failed write; the index holds no
    /// manifest bodies, so it stays small.
    async fn write(
        &self,
        change: impl FnOnce(&mut Index) -> Result<bool, StoreError>,
    ) -> Result<bool, StoreError> {
        let mut index = self.index.write().await;
        let mut next = index.clone();

        if !change(&mut next)? {
            return Ok(false);
        }
        self.storage.save(&next).await?;
        *index = next;
        Ok(true)
    }

    /// Drops the bodies of manifests no repository has any more. Runs under the write
    /// lock, so a concurrent put of the same manifest cannot lose its body. A failure
    /// leaves an unused body behind, which is harmless; fsck finds it.
    async fn drop_unused_bodies(&self, index: &Index, digests: &[Digest]) {
        for digest in digests.iter().filter(|d| !index.manifest_in_use(d)) {
            if let Err(err) = self.storage.delete_body(digest).await {
                tracing::warn!(%digest, %err, "cannot delete manifest body");
            }
        }
    }
}

impl<S: IndexStorage> RegistryMeta for IndexedMeta<S> {
    async fn list_repos(
        &self,
        after: Option<&RepoName>,
        limit: usize,
    ) -> Result<Vec<RepoSummary>, StoreError> {
        Ok(self.index.read().await.list_repos(after, limit))
    }

    async fn get_repo(&self, repo: &RepoName) -> Result<Option<RepoSummary>, StoreError> {
        Ok(self.index.read().await.get_repo(repo))
    }

    async fn set_repo_public(&self, repo: &RepoName, public: bool) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.set_repo_public(repo, public))).await
    }

    async fn delete_repo(&self, repo: &RepoName) -> Result<bool, StoreError> {
        let mut index = self.index.write().await;
        let mut next = index.clone();

        let Some(manifests) = next.delete_repo(repo) else {
            return Ok(false);
        };
        self.storage.save(&next).await?;
        *index = next;

        self.drop_unused_bodies(&index, &manifests).await;
        Ok(true)
    }

    async fn get_manifest(
        &self,
        repo: &RepoName,
        reference: &Reference,
    ) -> Result<Option<Manifest>, StoreError> {
        let Some((digest, media_type)) = self.index.read().await.resolve(repo, reference) else {
            return Ok(None);
        };

        // Read outside the lock; a body deleted meanwhile means the manifest is gone.
        Ok(self.storage.get_body(&digest).await?.map(|body| Manifest {
            digest,
            media_type,
            body,
        }))
    }

    async fn put_manifest(
        &self,
        repo: &RepoName,
        manifest: &NewManifest,
        tag: Option<&Tag>,
    ) -> Result<(), StoreError> {
        let mut index = self.index.write().await;

        // The body first: an index entry must never point at a missing body.
        self.storage
            .put_body(&manifest.digest, &manifest.body)
            .await?;

        let mut next = index.clone();
        next.put_manifest(repo, manifest, tag);
        self.storage.save(&next).await?;
        *index = next;
        Ok(())
    }

    async fn delete_manifest(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        let mut index = self.index.write().await;
        let mut next = index.clone();

        if !next.delete_manifest(repo, digest) {
            return Ok(false);
        }
        self.storage.save(&next).await?;
        *index = next;

        self.drop_unused_bodies(&index, &[*digest]).await;
        Ok(true)
    }

    async fn list_manifests(
        &self,
        repo: &RepoName,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Option<Vec<ManifestSummary>>, StoreError> {
        Ok(self.index.read().await.list_manifests(repo, after, limit))
    }

    async fn list_tags(
        &self,
        repo: &RepoName,
        after: Option<&Tag>,
        limit: usize,
    ) -> Result<Option<Vec<TagEntry>>, StoreError> {
        Ok(self.index.read().await.list_tags(repo, after, limit))
    }

    async fn delete_tag(&self, repo: &RepoName, tag: &Tag) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.delete_tag(repo, tag))).await
    }

    async fn list_referrers(
        &self,
        repo: &RepoName,
        subject: &Digest,
    ) -> Result<Vec<ManifestSummary>, StoreError> {
        Ok(self.index.read().await.list_referrers(repo, subject))
    }

    async fn repo_has_blob(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError> {
        Ok(self.index.read().await.repo_has_blob(repo, digest))
    }

    async fn referenced_blobs(&self) -> Result<Vec<Digest>, StoreError> {
        Ok(self.index.read().await.referenced_blobs())
    }
}

impl<S: IndexStorage> AuthMeta for IndexedMeta<S> {
    async fn get_user(&self, name: &UserName) -> Result<Option<User>, StoreError> {
        Ok(self.index.read().await.users.get(name).cloned())
    }

    async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        Ok(self.index.read().await.users.values().cloned().collect())
    }

    async fn create_user(&self, user: &User) -> Result<(), StoreError> {
        self.write(|i| i.create_user(user).map(|()| true))
            .await
            .map(drop)
    }

    async fn update_user(&self, user: &User) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.update_user(user))).await
    }

    async fn delete_user(&self, name: &UserName) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.delete_user(name))).await
    }

    async fn create_token(&self, token: &TokenRecord) -> Result<(), StoreError> {
        self.write(|i| i.create_token(token).map(|()| true))
            .await
            .map(drop)
    }

    async fn get_token_by_hash(&self, hash: &TokenHash) -> Result<Option<TokenRecord>, StoreError> {
        Ok(self.index.read().await.get_token_by_hash(hash))
    }

    async fn list_tokens(&self, user: Option<&UserName>) -> Result<Vec<TokenRecord>, StoreError> {
        Ok(self.index.read().await.list_tokens(user))
    }

    async fn revoke_token(&self, id: &TokenId) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.tokens.remove(id).is_some())).await
    }

    async fn set_token_last_used(&self, id: &TokenId, at: UnixTime) -> Result<bool, StoreError> {
        self.write(|i| Ok(i.set_token_last_used(id, at))).await
    }
}
