//! Metadata storage, split by concern into traits that one backend implements
//! together, so a single-writer backend keeps one lock over all of them.
//!
//! Methods acting on an existing record return whether it existed; creating a
//! record that is already there fails with `AlreadyExists`. Backends stamp times.

use super::{StoreError, UnixTime};
use crate::oci::{Digest, ParseError, Reference, RepoName, Tag};
use bytes::Bytes;
use std::str::FromStr;

/// A blob a manifest points at (config or layer), with the size from its descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRef {
    pub digest: Digest,
    pub size: u64,
}

/// A manifest as accepted by the core: the exact bytes plus what the core extracted
/// while validating them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewManifest {
    pub digest: Digest,
    pub media_type: String,
    pub body: Bytes,
    /// `config` and `layers`.
    pub blobs: Vec<BlobRef>,
    /// `manifests[]` of an index.
    pub children: Vec<Digest>,
    /// `subject`, for the referrers API.
    pub subject: Option<Digest>,
    pub artifact_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub digest: Digest,
    pub media_type: String,
    pub body: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSummary {
    pub digest: Digest,
    pub media_type: String,
    pub artifact_type: Option<String>,
    /// Size of the manifest body.
    pub size: u64,
    /// Tags pointing at it, sorted.
    pub tags: Vec<Tag>,
    pub created: UnixTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    pub name: RepoName,
    pub public: bool,
    pub tag_count: usize,
    pub manifest_count: usize,
    /// Sum of the distinct blobs the repository's manifests point at.
    pub size: u64,
    pub updated: UnixTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub tag: Tag,
    pub digest: Digest,
    pub updated: UnixTime,
}

/// Repositories, manifests and tags. A repository appears with its first manifest
/// and stays until `delete_repo`, also when it has no manifests left.
pub trait RegistryMeta {
    /// Ordered by name, starting after `after`.
    async fn list_repos(
        &self,
        after: Option<&RepoName>,
        limit: usize,
    ) -> Result<Vec<RepoSummary>, StoreError>;

    async fn get_repo(&self, repo: &RepoName) -> Result<Option<RepoSummary>, StoreError>;

    async fn set_repo_public(&self, repo: &RepoName, public: bool) -> Result<bool, StoreError>;

    /// Drops the repository with its manifests and tags. Blobs stay until GC.
    async fn delete_repo(&self, repo: &RepoName) -> Result<bool, StoreError>;

    async fn get_manifest(
        &self,
        repo: &RepoName,
        reference: &Reference,
    ) -> Result<Option<Manifest>, StoreError>;

    /// Stores the manifest and, if given, points the tag at it, as one operation.
    /// Storing a manifest that is already there only moves the tag.
    async fn put_manifest(
        &self,
        repo: &RepoName,
        manifest: &NewManifest,
        tag: Option<&Tag>,
    ) -> Result<(), StoreError>;

    /// Drops the manifest and the tags pointing at it.
    async fn delete_manifest(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError>;

    /// Ordered by digest, starting after `after`. `None` if the repository does not exist.
    async fn list_manifests(
        &self,
        repo: &RepoName,
        after: Option<&Digest>,
        limit: usize,
    ) -> Result<Option<Vec<ManifestSummary>>, StoreError>;

    /// Ordered by tag (lexically, as the spec requires), starting after `after`.
    /// `None` if the repository does not exist.
    async fn list_tags(
        &self,
        repo: &RepoName,
        after: Option<&Tag>,
        limit: usize,
    ) -> Result<Option<Vec<TagEntry>>, StoreError>;

    async fn delete_tag(&self, repo: &RepoName, tag: &Tag) -> Result<bool, StoreError>;

    /// Manifests in the repository whose `subject` is `subject`, ordered by digest.
    async fn list_referrers(
        &self,
        repo: &RepoName,
        subject: &Digest,
    ) -> Result<Vec<ManifestSummary>, StoreError>;

    /// Whether a manifest in the repository points at the blob: the pull access check.
    async fn repo_has_blob(&self, repo: &RepoName, digest: &Digest) -> Result<bool, StoreError>;

    /// Every blob some manifest points at, sorted and distinct: the GC mark phase.
    async fn referenced_blobs(&self) -> Result<Vec<Digest>, StoreError>;
}

/// `[a-z0-9][a-z0-9._-]{0,63}`: lowercase like registry logins, safe as a storage key.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserName(String);

impl FromStr for UserName {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();

        let ok = match s.as_bytes() {
            [first, rest @ ..] => {
                lower(first)
                    && rest.len() < 64
                    && rest.iter().all(|b| lower(b) || b"._-".contains(b))
            }
            [] => false,
        };

        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::new("user name", s))
        }
    }
}

/// The public part of a PAT (`thr_<id>_<secret>`), shown in the UI and used to revoke it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(String);

impl FromStr for TokenId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::new("token id", s))
        }
    }
}

crate::oci::display_as_str!(UserName, TokenId);

/// `sha256` of the whole token. The token itself is never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash(pub [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: UserName,
    pub admin: bool,
    pub created: UnixTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: TokenId,
    pub hash: TokenHash,
    pub user: UserName,
    pub description: String,
    /// `repository:<glob>:<actions>` and `registry:catalog:*`, kept as written;
    /// parsing and matching belong to `auth` (THL-43).
    pub scopes: Vec<String>,
    pub expires: Option<UnixTime>,
    pub created: UnixTime,
    pub last_used: Option<UnixTime>,
}

/// Users and personal access tokens. Who may do what is decided by `auth`; this
/// only stores the records.
pub trait AuthMeta {
    async fn get_user(&self, name: &UserName) -> Result<Option<User>, StoreError>;

    /// Ordered by name.
    async fn list_users(&self) -> Result<Vec<User>, StoreError>;

    async fn create_user(&self, user: &User) -> Result<(), StoreError>;

    async fn update_user(&self, user: &User) -> Result<bool, StoreError>;

    /// Drops the user with all their tokens.
    async fn delete_user(&self, name: &UserName) -> Result<bool, StoreError>;

    /// Fails with `AlreadyExists` if the id or the hash is taken, with `UserUnknown`
    /// if the owner does not exist.
    async fn create_token(&self, token: &TokenRecord) -> Result<(), StoreError>;

    async fn get_token_by_hash(&self, hash: &TokenHash) -> Result<Option<TokenRecord>, StoreError>;

    /// Tokens of one user, or of everyone with `None`; ordered by id.
    async fn list_tokens(&self, user: Option<&UserName>) -> Result<Vec<TokenRecord>, StoreError>;

    async fn revoke_token(&self, id: &TokenId) -> Result<bool, StoreError>;

    /// Called on token use; the caller batches calls, `/token` is hit several times
    /// per client operation.
    async fn set_token_last_used(&self, id: &TokenId, at: UnixTime) -> Result<bool, StoreError>;
}
