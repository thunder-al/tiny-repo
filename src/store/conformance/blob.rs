use super::{TempDir, fixture, sha256, sha512};
use crate::oci::{Digest, UploadId};
use crate::store::{BlobRead, BlobStore, ByteRange, StagedFile, Staging, StoreError};
use tokio::io::AsyncReadExt;

const MIB: usize = 1024 * 1024;

/// A store under test with a staging directory on the same filesystem, as `put`
/// needs for backends that move the file in.
pub struct Blobs<S> {
    pub store: S,
    pub staging: Staging,
    _dir: TempDir,
}

impl<S> Blobs<S> {
    /// Staging in a fresh temp dir, for stores that do not live on disk.
    pub async fn new(store: S) -> Self {
        let dir = TempDir::new();
        let staging = Staging::open(dir.path().join("uploads")).await.unwrap();
        Self::in_dir(store, staging, dir)
    }

    pub fn in_dir(store: S, staging: Staging, dir: TempDir) -> Self {
        Self {
            store,
            staging,
            _dir: dir,
        }
    }
}

impl<S> AsRef<Blobs<S>> for Blobs<S> {
    fn as_ref(&self) -> &Self {
        self
    }
}

fn id(s: &str) -> UploadId {
    s.parse().unwrap()
}

async fn read(blob: BlobRead) -> Vec<u8> {
    let mut out = Vec::new();
    let mut body = blob.body;
    body.read_to_end(&mut out).await.unwrap();
    out
}

async fn get_all(store: &impl BlobStore, digest: &Digest) -> Vec<u8> {
    read(store.get(digest, None).await.unwrap().unwrap()).await
}

async fn stage(staging: &Staging, upload_id: &str, data: &[u8]) -> StagedFile {
    let upload_id = id(upload_id);
    staging.create(&upload_id).await.unwrap();
    staging.append(&upload_id, data).await.unwrap();
    staging.finish(&upload_id).await.unwrap()
}

/// Stages `data` and puts it under its sha256.
async fn upload(t: &Blobs<impl BlobStore>, upload_id: &str, data: &[u8]) -> Digest {
    let digest = sha256(data);
    let file = stage(&t.staging, upload_id, data).await;
    t.store.put(&digest, file).await.unwrap();
    digest
}

pub async fn missing_blob(t: &Blobs<impl BlobStore>) {
    let digest = sha256(b"nothing");

    assert_eq!(t.store.stat(&digest).await.unwrap(), None);
    assert!(t.store.get(&digest, None).await.unwrap().is_none());
    assert!(!t.store.delete(&digest).await.unwrap());
}

pub async fn put_and_read(t: &Blobs<impl BlobStore>) {
    let data = fixture(4096, 1);
    let file = stage(&t.staging, "u1", &data).await;
    let staged = file.path().to_owned();
    let digest = sha256(&data);
    t.store.put(&digest, file).await.unwrap();

    assert_eq!(t.store.stat(&digest).await.unwrap(), Some(4096));
    let blob = t.store.get(&digest, None).await.unwrap().unwrap();
    assert_eq!(blob.size, 4096);
    assert_eq!(read(blob).await, data);
    assert!(!staged.exists(), "the staged file is consumed");
}

/// Crosses the 16 MiB part size of the S3 backend.
pub async fn large_blob(t: &Blobs<impl BlobStore>) {
    let data = fixture(20 * MIB + 7, 4);
    let digest = upload(t, "u1", &data).await;

    assert_eq!(
        t.store.stat(&digest).await.unwrap(),
        Some(data.len() as u64)
    );
    assert!(get_all(&t.store, &digest).await == data);
}

pub async fn empty_blob(t: &Blobs<impl BlobStore>) {
    let digest = upload(t, "u1", &[]).await;

    assert_eq!(t.store.stat(&digest).await.unwrap(), Some(0));
    assert!(get_all(&t.store, &digest).await.is_empty());
    assert!(matches!(
        t.store
            .get(
                &digest,
                Some(ByteRange {
                    start: 0,
                    end: None
                })
            )
            .await,
        Err(StoreError::RangeNotSatisfiable)
    ));
}

pub async fn range_reads(t: &Blobs<impl BlobStore>) {
    let data = fixture(1000, 5);
    let digest = upload(t, "u1", &data).await;

    let cases = [
        (10, Some(19), &data[10..20]),
        (990, None, &data[990..]),
        (500, Some(5000), &data[500..]),
        (999, Some(999), &data[999..]),
        (0, None, &data[..]),
    ];
    for (start, end, expected) in cases {
        let blob = t
            .store
            .get(&digest, Some(ByteRange { start, end }))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            blob.size, 1000,
            "size is the whole blob for {start}-{end:?}"
        );
        assert_eq!(read(blob).await, expected, "range {start}-{end:?}");
    }

    for (start, end) in [(1000, None), (1500, Some(2000)), (20, Some(10))] {
        let result = t.store.get(&digest, Some(ByteRange { start, end })).await;
        assert!(
            matches!(result, Err(StoreError::RangeNotSatisfiable)),
            "range {start}-{end:?}"
        );
    }
}

pub async fn sha512_blob(t: &Blobs<impl BlobStore>) {
    let data = fixture(2048, 6);
    let digest = sha512(&data);
    let file = stage(&t.staging, "u1", &data).await;
    t.store.put(&digest, file).await.unwrap();

    assert_eq!(get_all(&t.store, &digest).await, data);
    assert_eq!(t.store.stat(&sha256(&data)).await.unwrap(), None);
}

pub async fn put_existing_blob_keeps_it(t: &Blobs<impl BlobStore>) {
    let data = fixture(512, 7);
    let digest = upload(t, "u1", &data).await;

    let again = stage(&t.staging, "u2", &data).await;
    let staged = again.path().to_owned();
    t.store.put(&digest, again).await.unwrap();

    assert_eq!(get_all(&t.store, &digest).await, data);
    assert!(!staged.exists(), "the duplicate is dropped");
    assert_eq!(t.store.list(None, 10).await.unwrap().len(), 1);
}

pub async fn concurrent_puts_of_same_blob(t: &Blobs<impl BlobStore>) {
    let data = fixture(MIB, 8);
    let digest = sha256(&data);
    let a = stage(&t.staging, "u1", &data).await;
    let b = stage(&t.staging, "u2", &data).await;

    let (ra, rb) = tokio::join!(t.store.put(&digest, a), t.store.put(&digest, b));
    ra.unwrap();
    rb.unwrap();

    assert_eq!(get_all(&t.store, &digest).await, data);
    assert_eq!(t.store.list(None, 10).await.unwrap().len(), 1);
}

pub async fn delete_blob(t: &Blobs<impl BlobStore>) {
    let digest = upload(t, "u1", &fixture(64, 13)).await;

    assert!(t.store.delete(&digest).await.unwrap());
    assert_eq!(t.store.stat(&digest).await.unwrap(), None);
    assert!(!t.store.delete(&digest).await.unwrap());
}

pub async fn list_paginates_in_digest_order(t: &Blobs<impl BlobStore>) {
    let mut expected = Vec::new();
    for seed in 0..5 {
        let data = fixture(100 + seed, 100 + seed as u64);
        let digest = upload(t, &format!("u{seed}"), &data).await;
        expected.push((digest, data.len() as u64));
    }
    let data = fixture(77, 200);
    let digest = sha512(&data);
    let file = stage(&t.staging, "u512", &data).await;
    t.store.put(&digest, file).await.unwrap();
    expected.push((digest, 77));
    expected.sort();

    let mut listed = Vec::new();
    let mut after = None;
    loop {
        let page = t.store.list(after.as_ref(), 4).await.unwrap();
        assert!(page.len() <= 4);
        let Some(last) = page.last() else { break };
        after = Some(last.digest);
        listed.extend(page.into_iter().map(|e| (e.digest, e.size)));
    }

    assert_eq!(listed, expected);
}
