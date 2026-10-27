//! Blob upload sessions. A session lives in memory (repository, both hashers, size,
//! last activity) with its bytes in `Staging`; the digest is checked here before the
//! file goes to `BlobStore::put`. The hashers die with the process, so a session is
//! only usable by the process that started it: after a restart the client gets
//! `Unknown` and uploads the blob again.
//!
//! Abandoned sessions are dropped lazily, on the next `start`, not by a background
//! task and not on startup: another process may own files in the same directory.

use crate::oci::{Digest, RepoName, UploadId};
use crate::store::{BlobStore, RegistryMeta, Staging, Storage, StoreError};
use sha2::{Digest as _, Sha256, Sha512};
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("upload session not found")]
    Unknown,
    #[error("chunk starts at byte {start}, the upload has {size} bytes")]
    OutOfOrder { size: u64, start: u64 },
    #[error("blob exceeds the limit of {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("upload body broke off: {0}")]
    Body(io::Error),
    #[error("content does not match {0}")]
    DigestMismatch(Digest),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct Uploads {
    sessions: Mutex<HashMap<UploadId, Arc<tokio::sync::Mutex<Session>>>>,
    max_blob_size: Option<u64>,
    ttl: Duration,
}

struct Session {
    repo: RepoName,
    sha256: Sha256,
    sha512: Sha512,
    size: u64,
    last_active: Instant,
    /// False once finished or cancelled; a request that waited for the lock sees it.
    open: bool,
}

impl Uploads {
    /// `max_blob_size: None` accepts blobs of any size.
    pub fn new(max_blob_size: Option<u64>, ttl: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            max_blob_size,
            ttl,
        }
    }

    pub async fn start(&self, staging: &Staging, repo: &RepoName) -> Result<UploadId, UploadError> {
        self.drop_stale(staging).await;

        let id = loop {
            let id = new_id()?;
            match staging.create(&id).await {
                Ok(()) => break id,
                // 128 random bits do not collide; this only guards the invariant.
                Err(StoreError::AlreadyExists) => continue,
                Err(e) => return Err(e.into()),
            }
        };

        let session = Session {
            repo: repo.clone(),
            sha256: Sha256::new(),
            sha512: Sha512::new(),
            size: 0,
            last_active: Instant::now(),
            open: true,
        };
        self.sessions()
            .insert(id.clone(), Arc::new(tokio::sync::Mutex::new(session)));
        Ok(id)
    }

    /// Appends `body` and returns the new size. `start` is where the client says the
    /// chunk begins (`Content-Range`); it has to be the current size. Any failure
    /// while writing ends the session: what reached the file is unknown.
    pub async fn append(
        &self,
        staging: &Staging,
        repo: &RepoName,
        id: &UploadId,
        start: Option<u64>,
        body: impl AsyncRead + Send + Unpin,
    ) -> Result<u64, UploadError> {
        let session = self.session(id)?;
        let mut s = session.lock().await;
        check(&s, repo)?;

        if let Some(start) = start
            && start != s.size
        {
            return Err(UploadError::OutOfOrder {
                size: s.size,
                start,
            });
        }

        self.write(staging, id, &mut s, body).await?;
        Ok(s.size)
    }

    /// Bytes received so far.
    pub async fn size(&self, repo: &RepoName, id: &UploadId) -> Result<u64, UploadError> {
        let session = self.session(id)?;
        let s = session.lock().await;
        check(&s, repo)?;
        Ok(s.size)
    }

    /// Appends the last chunk, checks the digest and stores the blob. The session
    /// ends either way.
    pub async fn finish(
        &self,
        storage: &Storage,
        repo: &RepoName,
        id: &UploadId,
        digest: &Digest,
        tail: impl AsyncRead + Send + Unpin,
    ) -> Result<(), UploadError> {
        let session = self.session(id)?;
        let mut s = session.lock().await;
        check(&s, repo)?;

        self.write(&storage.staging, id, &mut s, tail).await?;

        let actual = match digest {
            Digest::Sha256(_) => Digest::Sha256(s.sha256.clone().finalize().into()),
            Digest::Sha512(_) => Digest::Sha512(s.sha512.clone().finalize().into()),
        };
        if actual != *digest {
            self.close(&storage.staging, id, &mut s).await;
            return Err(UploadError::DigestMismatch(*digest));
        }

        let file = match storage.staging.finish(id).await {
            Ok(file) => file,
            Err(e) => {
                self.close(&storage.staging, id, &mut s).await;
                return Err(e.into());
            }
        };
        s.open = false;
        self.sessions().remove(id);

        // Pushing a blob that is already stored is common (shared base layers).
        if storage.blobs.stat(digest).await?.is_none() {
            storage.blobs.put(digest, file).await?;
        }
        Ok(())
    }

    pub async fn cancel(
        &self,
        staging: &Staging,
        repo: &RepoName,
        id: &UploadId,
    ) -> Result<(), UploadError> {
        let session = self.session(id)?;
        let mut s = session.lock().await;
        check(&s, repo)?;
        self.close(staging, id, &mut s).await;
        Ok(())
    }

    /// Streams `body` into the staged file through the hashers.
    async fn write(
        &self,
        staging: &Staging,
        id: &UploadId,
        s: &mut Session,
        body: impl AsyncRead + Send + Unpin,
    ) -> Result<(), UploadError> {
        let mut reader = Hashing {
            inner: body,
            sha256: &mut s.sha256,
            sha512: &mut s.sha512,
            size: &mut s.size,
            limit: self.max_blob_size,
            failure: None,
        };
        let result = staging.append(id, &mut reader).await;
        let failure = reader.failure.take();
        s.last_active = Instant::now();

        let Err(err) = result else {
            return Ok(());
        };
        self.close(staging, id, s).await;
        Err(match failure {
            Some(Failure::TooLarge(limit)) => UploadError::TooLarge { limit },
            Some(Failure::Body(e)) => UploadError::Body(e),
            None => err.into(),
        })
    }

    /// Ends the session and removes its file. A failure to remove leaves a file that
    /// the next `drop_stale` takes.
    async fn close(&self, staging: &Staging, id: &UploadId, s: &mut Session) {
        s.open = false;
        self.sessions().remove(id);
        if let Err(err) = staging.abort(id).await {
            tracing::warn!(%id, %err, "cannot remove staged upload");
        }
    }

    /// Drops sessions idle for longer than the TTL, and staged files no session here
    /// knows (left by a previous process) that were not written for that long. A
    /// session with a request in progress holds its lock and is skipped.
    async fn drop_stale(&self, staging: &Staging) {
        let sessions: Vec<_> = self
            .sessions()
            .iter()
            .map(|(id, s)| (id.clone(), Arc::clone(s)))
            .collect();
        for (id, session) in sessions {
            let Ok(mut s) = session.try_lock() else {
                continue;
            };
            if s.open && s.last_active.elapsed() > self.ttl {
                tracing::info!(%id, repo = %s.repo, "dropping idle upload");
                self.close(staging, &id, &mut s).await;
            }
        }

        let entries = match staging.list().await {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!(%err, "cannot list staged uploads");
                return;
            }
        };
        for entry in entries {
            let idle = entry.modified.elapsed().unwrap_or_default();
            if idle > self.ttl && !self.sessions().contains_key(&entry.id) {
                tracing::info!(id = %entry.id, "dropping orphaned upload");
                if let Err(err) = staging.abort(&entry.id).await {
                    tracing::warn!(id = %entry.id, %err, "cannot remove staged upload");
                }
            }
        }
    }

    fn session(&self, id: &UploadId) -> Result<Arc<tokio::sync::Mutex<Session>>, UploadError> {
        self.sessions().get(id).cloned().ok_or(UploadError::Unknown)
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<UploadId, Arc<tokio::sync::Mutex<Session>>>> {
        // Held only for map lookups, never across an await; a panic elsewhere does
        // not leave the map half-updated.
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether `repo` may use the session: ids are per repository.
fn check(s: &Session, repo: &RepoName) -> Result<(), UploadError> {
    if s.open && s.repo == *repo {
        Ok(())
    } else {
        Err(UploadError::Unknown)
    }
}

/// 128 random bits as hex: not guessable, so a session cannot be written to by a
/// client that did not start it.
fn new_id() -> Result<UploadId, UploadError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| StoreError::Io(io::Error::other(e)))?;
    crate::oci::encode_hex(&bytes)
        .parse()
        .map_err(|e| StoreError::Io(io::Error::other(e)).into())
}

/// Whether a blob can be mounted from `from` instead of uploaded: it is stored and
/// a manifest in `from` points at it.
pub async fn mountable(
    storage: &Storage,
    from: &RepoName,
    digest: &Digest,
) -> Result<bool, StoreError> {
    Ok(storage.meta.repo_has_blob(from, digest).await?
        && storage.blobs.stat(digest).await?.is_some())
}

enum Failure {
    TooLarge(u64),
    Body(io::Error),
}

/// Feeds every byte read from the body into both hashers and enforces the size
/// limit. Remembers why reading failed, so a broken client and a full disk can be
/// told apart after `Staging::append` returns.
struct Hashing<'a, R> {
    inner: R,
    sha256: &'a mut Sha256,
    sha512: &'a mut Sha512,
    size: &'a mut u64,
    limit: Option<u64>,
    failure: Option<Failure>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Hashing<'_, R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();

        if let Err(e) = std::task::ready!(Pin::new(&mut this.inner).poll_read(cx, buf)) {
            let err = io::Error::new(e.kind(), e.to_string());
            this.failure = Some(Failure::Body(e));
            return Poll::Ready(Err(err));
        }

        let chunk = &buf.filled()[before..];
        let size = *this.size + chunk.len() as u64;
        if let Some(limit) = this.limit
            && size > limit
        {
            this.failure = Some(Failure::TooLarge(limit));
            return Poll::Ready(Err(io::Error::other("blob size limit exceeded")));
        }

        this.sha256.update(chunk);
        this.sha512.update(chunk);
        *this.size = size;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::conformance::{TempDir, fixture, sha256};

    fn repo(s: &str) -> RepoName {
        s.parse().unwrap()
    }

    async fn staging() -> (Staging, TempDir) {
        let dir = TempDir::new();
        (
            Staging::open(dir.path().join("uploads")).await.unwrap(),
            dir,
        )
    }

    #[tokio::test]
    async fn ids_are_random_hex() {
        let (s, _dir) = staging().await;
        let uploads = Uploads::new(None, Duration::from_secs(60));
        let a = uploads.start(&s, &repo("app")).await.unwrap().to_string();
        let b = uploads.start(&s, &repo("app")).await.unwrap().to_string();

        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn session_belongs_to_its_repo() {
        let (s, _dir) = staging().await;
        let uploads = Uploads::new(None, Duration::from_secs(60));
        let id = uploads.start(&s, &repo("team/a")).await.unwrap();

        assert!(matches!(
            uploads
                .append(&s, &repo("team/b"), &id, None, &b"x"[..])
                .await,
            Err(UploadError::Unknown)
        ));
        assert_eq!(uploads.size(&repo("team/a"), &id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn chunk_must_start_at_current_size() {
        let (s, _dir) = staging().await;
        let uploads = Uploads::new(None, Duration::from_secs(60));
        let r = repo("app");
        let id = uploads.start(&s, &r).await.unwrap();
        uploads
            .append(&s, &r, &id, Some(0), &b"abcd"[..])
            .await
            .unwrap();

        assert!(matches!(
            uploads.append(&s, &r, &id, Some(2), &b"cd"[..]).await,
            Err(UploadError::OutOfOrder { size: 4, start: 2 })
        ));
        assert_eq!(
            uploads
                .append(&s, &r, &id, Some(4), &b"ef"[..])
                .await
                .unwrap(),
            6
        );
    }

    #[tokio::test]
    async fn size_limit_ends_session() {
        let (s, _dir) = staging().await;
        let uploads = Uploads::new(Some(1000), Duration::from_secs(60));
        let r = repo("app");
        let id = uploads.start(&s, &r).await.unwrap();
        uploads
            .append(&s, &r, &id, None, &fixture(600, 1)[..])
            .await
            .unwrap();

        assert!(matches!(
            uploads
                .append(&s, &r, &id, None, &fixture(600, 2)[..])
                .await,
            Err(UploadError::TooLarge { limit: 1000 })
        ));
        assert!(matches!(
            uploads.size(&r, &id).await,
            Err(UploadError::Unknown)
        ));
        assert!(s.list().await.unwrap().is_empty(), "staged file removed");
    }

    #[tokio::test]
    async fn idle_sessions_and_orphans_are_dropped_on_start() {
        let (s, _dir) = staging().await;
        let r = repo("app");
        // A file from a previous process: no session knows it.
        let orphan: UploadId = "0rphan".parse().unwrap();
        s.create(&orphan).await.unwrap();

        let uploads = Uploads::new(None, Duration::from_millis(50));
        let idle = uploads.start(&s, &r).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        let fresh = uploads.start(&s, &r).await.unwrap();

        assert!(matches!(
            uploads.size(&r, &idle).await,
            Err(UploadError::Unknown)
        ));
        assert_eq!(uploads.size(&r, &fresh).await.unwrap(), 0);
        let left: Vec<UploadId> = s.list().await.unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(left, [fresh]);
    }

    #[tokio::test]
    async fn fresh_orphans_are_kept() {
        let (s, _dir) = staging().await;
        // Another process may be filling this one right now.
        let foreign: UploadId = "foreign".parse().unwrap();
        s.create(&foreign).await.unwrap();

        let uploads = Uploads::new(None, Duration::from_secs(60));
        uploads.start(&s, &repo("app")).await.unwrap();

        assert!(s.list().await.unwrap().iter().any(|e| e.id == foreign));
    }

    #[tokio::test]
    async fn hashes_both_algorithms() {
        let data = fixture(5000, 3);
        let (mut a, mut b, mut size) = (Sha256::new(), Sha512::new(), 0);
        let mut reader = Hashing {
            inner: &data[..],
            sha256: &mut a,
            sha512: &mut b,
            size: &mut size,
            limit: None,
            failure: None,
        };
        tokio::io::copy(&mut reader, &mut tokio::io::sink())
            .await
            .unwrap();

        assert_eq!(size, 5000);
        assert_eq!(Digest::Sha256(a.finalize().into()), sha256(&data));
        assert_eq!(
            Digest::Sha512(b.finalize().into()),
            crate::store::conformance::sha512(&data)
        );
    }
}
