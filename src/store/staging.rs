//! Local scratch directory for blob uploads, the same for every backend: the body
//! of an upload is appended to `<dir>/<id>`, and once the core has checked the
//! digest the finished file goes to `BlobStore::put`. A file there is not a blob
//! yet and is never served.
//!
//! The directory is not durable state: after a restart the core's hashers are gone,
//! so the core drops the sessions it finds (`list` + `abort`) and clients start over.

use super::{StoreError, UnixTime};
use crate::oci::UploadId;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncRead, AsyncWriteExt, BufWriter};

/// Each write to a `tokio::fs::File` is a blocking-pool round trip, so bodies are
/// written in large pieces.
const WRITE_BUFFER: usize = 256 * 1024;

/// Suffix of files handed out by `finish`. Upload ids cannot contain `.`, so these
/// never look like a session.
const STAGED_SUFFIX: &str = ".staged";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadEntry {
    pub id: UploadId,
    pub started: UnixTime,
    /// Last write. A session another process is still filling keeps moving it.
    pub modified: SystemTime,
}

pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    /// Creates the directory if needed. Finished files left over from a crash have
    /// no owner any more and are removed; open sessions are kept for the core.
    pub async fn open(dir: PathBuf) -> Result<Self, StoreError> {
        fs::create_dir_all(&dir).await?;

        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name().to_string_lossy().ends_with(STAGED_SUFFIX) {
                remove_if_exists(&entry.path()).await?;
            }
        }

        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Fails with `AlreadyExists` if the id is taken.
    pub async fn create(&self, id: &UploadId) -> Result<(), StoreError> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path(id))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(StoreError::AlreadyExists),
            Err(e) => Err(e.into()),
        }
    }

    /// Appends the whole body and returns the number of bytes written. After an
    /// error the upload's content is undefined; the caller aborts it. Appends to one
    /// session are serialized by the caller.
    pub async fn append(
        &self,
        id: &UploadId,
        mut body: impl AsyncRead + Send + Unpin,
    ) -> Result<u64, StoreError> {
        let file = OpenOptions::new()
            .append(true)
            .open(self.path(id))
            .await
            .map_err(unknown_if_missing)?;

        let mut writer = BufWriter::with_capacity(WRITE_BUFFER, file);
        let written = tokio::io::copy(&mut body, &mut writer).await?;
        writer.flush().await?;
        Ok(written)
    }

    /// Bytes received so far, for the `Range` of an upload status.
    pub async fn size(&self, id: &UploadId) -> Result<u64, StoreError> {
        let meta = fs::metadata(self.path(id))
            .await
            .map_err(unknown_if_missing)?;
        Ok(meta.len())
    }

    /// Ends the session and hands out its content, flushed to disk. Later calls with
    /// this id fail with `UploadUnknown`.
    pub async fn finish(&self, id: &UploadId) -> Result<StagedFile, StoreError> {
        let path = self.path(id);
        let staged = self.dir.join(format!("{id}{STAGED_SUFFIX}"));

        fs::rename(&path, &staged)
            .await
            .map_err(unknown_if_missing)?;
        // Owned from here on, so an error below removes the file.
        let mut file = StagedFile {
            path: staged,
            size: 0,
        };

        let handle = File::open(&file.path).await?;
        handle.sync_all().await?;
        file.size = handle.metadata().await?.len();
        Ok(file)
    }

    /// Returns whether the upload existed.
    pub async fn abort(&self, id: &UploadId) -> Result<bool, StoreError> {
        Ok(remove_if_exists(&self.path(id)).await?)
    }

    /// Open sessions, for dropping them after a restart or when they go stale.
    pub async fn list(&self) -> Result<Vec<UploadEntry>, StoreError> {
        let mut out = Vec::new();
        let mut entries = fs::read_dir(&self.dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let Ok(id) = entry.file_name().to_string_lossy().parse::<UploadId>() else {
                continue;
            };
            let meta = match entry.metadata().await {
                Ok(meta) => meta,
                // Finished or aborted since `read_dir` saw it.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let modified = meta.modified()?;
            // Birth time where the filesystem records it (ext4, xfs, btrfs), else
            // the last write.
            let started = meta.created().unwrap_or(modified);

            out.push(UploadEntry {
                id,
                started: UnixTime::from_system(started),
                modified,
            });
        }
        Ok(out)
    }

    fn path(&self, id: &UploadId) -> PathBuf {
        self.dir.join(id.to_string())
    }
}

/// A finished upload waiting for `BlobStore::put`. Dropping it deletes the file, so
/// an upload that is not stored for any reason does not stay on disk.
pub struct StagedFile {
    path: PathBuf,
    size: u64,
}

impl StagedFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Moves the file to `dest`, which must be on the same filesystem.
    pub async fn rename_to(mut self, dest: &Path) -> io::Result<()> {
        fs::rename(&self.path, dest).await?;
        self.path = PathBuf::new();
        Ok(())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        // Drop cannot await; unlinking one file is a short syscall.
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %err, "cannot remove staged file");
        }
    }
}

fn unknown_if_missing(err: io::Error) -> StoreError {
    if err.kind() == io::ErrorKind::NotFound {
        StoreError::UploadUnknown
    } else {
        err.into()
    }
}

/// Returns whether the file existed.
pub(super) async fn remove_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::conformance::{TempDir, fixture};
    use std::io::Cursor;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncReadExt, ReadBuf};

    fn id(s: &str) -> UploadId {
        s.parse().unwrap()
    }

    async fn staging() -> (Staging, TempDir) {
        let dir = TempDir::new();
        (
            Staging::open(dir.path().join("uploads")).await.unwrap(),
            dir,
        )
    }

    async fn content(file: &StagedFile) -> Vec<u8> {
        let mut out = Vec::new();
        File::open(file.path())
            .await
            .unwrap()
            .read_to_end(&mut out)
            .await
            .unwrap();
        out
    }

    #[tokio::test]
    async fn chunks_concatenate() {
        let (s, _dir) = staging().await;
        let (a, b) = (fixture(1000, 1), fixture(300_000, 2));
        s.create(&id("u1")).await.unwrap();

        assert_eq!(s.append(&id("u1"), &a[..]).await.unwrap(), 1000);
        assert_eq!(s.append(&id("u1"), &b""[..]).await.unwrap(), 0);
        assert_eq!(s.append(&id("u1"), &b[..]).await.unwrap(), 300_000);
        assert_eq!(s.size(&id("u1")).await.unwrap(), 301_000);

        let file = s.finish(&id("u1")).await.unwrap();
        assert_eq!(file.size(), 301_000);
        assert_eq!(content(&file).await, [a, b].concat());
    }

    #[tokio::test]
    async fn finish_ends_session() {
        let (s, _dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        let _file = s.finish(&id("u1")).await.unwrap();

        assert!(matches!(
            s.append(&id("u1"), &b"x"[..]).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(matches!(
            s.finish(&id("u1")).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(!s.abort(&id("u1")).await.unwrap());
        assert!(s.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropped_file_is_removed() {
        let (s, _dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        let file = s.finish(&id("u1")).await.unwrap();
        let path = file.path().to_owned();

        assert!(path.exists());
        drop(file);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn rename_moves_file_out() {
        let (s, dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        s.append(&id("u1"), &b"data"[..]).await.unwrap();
        let file = s.finish(&id("u1")).await.unwrap();

        let dest = dir.path().join("moved");
        file.rename_to(&dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    }

    #[tokio::test]
    async fn sessions_are_isolated() {
        let (s, _dir) = staging().await;
        let (a, b) = (fixture(3000, 3), fixture(3000, 4));
        s.create(&id("a")).await.unwrap();
        s.create(&id("b")).await.unwrap();

        s.append(&id("a"), &a[..1500]).await.unwrap();
        s.append(&id("b"), &b[..1500]).await.unwrap();
        let (ua, ub) = (id("a"), id("b"));
        let (ra, rb) = tokio::join!(s.append(&ua, &a[1500..]), s.append(&ub, &b[1500..]));
        ra.unwrap();
        rb.unwrap();

        assert_eq!(content(&s.finish(&ua).await.unwrap()).await, a);
        assert_eq!(content(&s.finish(&ub).await.unwrap()).await, b);
    }

    #[tokio::test]
    async fn create_twice() {
        let (s, _dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        assert!(matches!(
            s.create(&id("u1")).await,
            Err(StoreError::AlreadyExists)
        ));
    }

    #[tokio::test]
    async fn unknown_upload() {
        let (s, _dir) = staging().await;
        let nope = id("nope");

        assert!(matches!(
            s.append(&nope, &b"x"[..]).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(matches!(
            s.size(&nope).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(matches!(
            s.finish(&nope).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(!s.abort(&nope).await.unwrap());
    }

    #[tokio::test]
    async fn abort_ends_upload() {
        let (s, _dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        s.append(&id("u1"), &b"data"[..]).await.unwrap();

        assert!(s.abort(&id("u1")).await.unwrap());
        assert!(!s.abort(&id("u1")).await.unwrap());
        assert!(matches!(
            s.append(&id("u1"), &b"x"[..]).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(s.list().await.unwrap().is_empty());
    }

    /// A client that drops the connection mid-body: the append fails, the session
    /// stays for the caller to abort.
    #[tokio::test]
    async fn failed_append_returns_error() {
        struct Broken;

        impl AsyncRead for Broken {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                _: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "client went away",
                )))
            }
        }

        let (s, _dir) = staging().await;
        s.create(&id("u1")).await.unwrap();
        let body = Cursor::new(fixture(5000, 5)).chain(Broken);

        assert!(matches!(
            s.append(&id("u1"), body).await,
            Err(StoreError::Io(_))
        ));
        assert!(s.abort(&id("u1")).await.unwrap());
    }

    #[tokio::test]
    async fn list_tracks_open_sessions() {
        let (s, _dir) = staging().await;
        let before = UnixTime::now();
        for u in ["a", "b", "c"] {
            s.create(&id(u)).await.unwrap();
        }
        let _finished = s.finish(&id("a")).await.unwrap();
        s.abort(&id("b")).await.unwrap();

        let open = s.list().await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, id("c"));
        assert!(open[0].started >= before);
    }

    #[tokio::test]
    async fn reopen_keeps_sessions_and_drops_finished_files() {
        let dir = TempDir::new();
        let path = dir.path().join("uploads");
        let s = Staging::open(path.clone()).await.unwrap();
        s.create(&id("open")).await.unwrap();
        s.append(&id("open"), &b"part"[..]).await.unwrap();
        s.create(&id("done")).await.unwrap();
        let finished = s.finish(&id("done")).await.unwrap();
        let finished_path = finished.path().to_owned();
        // A crash between `finish` and `put`: the file stays behind.
        std::mem::forget(finished);

        let s = Staging::open(path).await.unwrap();
        assert!(!finished_path.exists());
        let open: Vec<UploadId> = s.list().await.unwrap().into_iter().map(|u| u.id).collect();
        assert_eq!(open, [id("open")]);
        assert_eq!(s.size(&id("open")).await.unwrap(), 4);
    }
}
