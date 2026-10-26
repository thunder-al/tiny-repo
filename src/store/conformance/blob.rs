use super::{fixture, sha256, sha512};
use crate::oci::{Digest, UploadId};
use crate::store::{BlobRead, BlobStore, ByteRange, StoreError};
use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

const MIB: usize = 1024 * 1024;

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

/// Uploads `chunks` as separate appends and commits under their sha256.
async fn upload(store: &impl BlobStore, upload_id: &str, chunks: &[&[u8]]) -> Digest {
    let upload_id = id(upload_id);
    store.start_upload(&upload_id).await.unwrap();

    let mut all = Vec::new();
    for chunk in chunks {
        let written = store.append(&upload_id, *chunk).await.unwrap();
        assert_eq!(written, chunk.len() as u64);
        all.extend_from_slice(chunk);
    }

    let digest = sha256(&all);
    store.commit(&upload_id, &digest).await.unwrap();
    digest
}

pub async fn missing_blob(store: &impl BlobStore) {
    let digest = sha256(b"nothing");

    assert_eq!(store.stat(&digest).await.unwrap(), None);
    assert!(store.get(&digest, None).await.unwrap().is_none());
    assert!(!store.delete(&digest).await.unwrap());
}

pub async fn upload_and_read(store: &impl BlobStore) {
    let data = fixture(4096, 1);
    let digest = upload(store, "u1", &[&data]).await;

    assert_eq!(store.stat(&digest).await.unwrap(), Some(4096));
    let blob = store.get(&digest, None).await.unwrap().unwrap();
    assert_eq!(blob.size, 4096);
    assert_eq!(read(blob).await, data);
}

pub async fn chunks_concatenate(store: &impl BlobStore) {
    let (a, b) = (fixture(1000, 2), fixture(3000, 3));
    let digest = upload(store, "u1", &[&a, b"", &b]).await;

    assert_eq!(get_all(store, &digest).await, [a, b].concat());
}

/// Crosses the 16 MiB part size of the S3 backend with uneven appends.
pub async fn large_upload(store: &impl BlobStore) {
    let data = fixture(20 * MIB + 7, 4);
    let (a, rest) = data.split_at(7 * MIB + 1);
    let (b, c) = rest.split_at(9 * MIB);
    let digest = upload(store, "u1", &[a, b, c]).await;

    assert_eq!(store.stat(&digest).await.unwrap(), Some(data.len() as u64));
    assert!(get_all(store, &digest).await == data);
}

pub async fn empty_blob(store: &impl BlobStore) {
    let digest = upload(store, "u1", &[]).await;

    assert_eq!(store.stat(&digest).await.unwrap(), Some(0));
    assert!(get_all(store, &digest).await.is_empty());
    assert!(matches!(
        store
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

pub async fn range_reads(store: &impl BlobStore) {
    let data = fixture(1000, 5);
    let digest = upload(store, "u1", &[&data]).await;

    let cases = [
        (10, Some(19), &data[10..20]),
        (990, None, &data[990..]),
        (500, Some(5000), &data[500..]),
        (999, Some(999), &data[999..]),
        (0, None, &data[..]),
    ];
    for (start, end, expected) in cases {
        let blob = store
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
        let result = store.get(&digest, Some(ByteRange { start, end })).await;
        assert!(
            matches!(result, Err(StoreError::RangeNotSatisfiable)),
            "range {start}-{end:?}"
        );
    }
}

pub async fn sha512_blob(store: &impl BlobStore) {
    let data = fixture(2048, 6);
    let digest = sha512(&data);

    store.start_upload(&id("u1")).await.unwrap();
    store.append(&id("u1"), &data[..]).await.unwrap();
    store.commit(&id("u1"), &digest).await.unwrap();

    assert_eq!(get_all(store, &digest).await, data);
    assert_eq!(store.stat(&sha256(&data)).await.unwrap(), None);
}

pub async fn commit_existing_blob_keeps_it(store: &impl BlobStore) {
    let data = fixture(512, 7);
    let digest = upload(store, "u1", &[&data]).await;
    let again = upload(store, "u2", &[&data]).await;

    assert_eq!(digest, again);
    assert_eq!(get_all(store, &digest).await, data);
    assert!(store.list_uploads().await.unwrap().is_empty());
    assert_eq!(store.list(None, 10).await.unwrap().len(), 1);
}

pub async fn concurrent_commits_of_same_blob(store: &impl BlobStore) {
    let data = fixture(MIB, 8);
    let digest = sha256(&data);

    for u in ["u1", "u2"] {
        store.start_upload(&id(u)).await.unwrap();
        store.append(&id(u), &data[..]).await.unwrap();
    }
    let (u1, u2) = (id("u1"), id("u2"));
    let (a, b) = tokio::join!(store.commit(&u1, &digest), store.commit(&u2, &digest));
    a.unwrap();
    b.unwrap();

    assert_eq!(get_all(store, &digest).await, data);
}

pub async fn concurrent_uploads_are_isolated(store: &impl BlobStore) {
    let (a, b) = (fixture(3000, 9), fixture(3000, 10));
    store.start_upload(&id("a")).await.unwrap();
    store.start_upload(&id("b")).await.unwrap();

    // Interleaved halves, then both finish at once.
    store.append(&id("a"), &a[..1500]).await.unwrap();
    store.append(&id("b"), &b[..1500]).await.unwrap();
    let (ua, ub) = (id("a"), id("b"));
    let (ra, rb) = tokio::join!(store.append(&ua, &a[1500..]), store.append(&ub, &b[1500..]));
    ra.unwrap();
    rb.unwrap();

    let (da, db) = (sha256(&a), sha256(&b));
    store.commit(&id("a"), &da).await.unwrap();
    store.commit(&id("b"), &db).await.unwrap();

    assert_eq!(get_all(store, &da).await, a);
    assert_eq!(get_all(store, &db).await, b);
}

pub async fn abort_ends_upload(store: &impl BlobStore) {
    let data = fixture(100, 11);
    store.start_upload(&id("u1")).await.unwrap();
    store.append(&id("u1"), &data[..]).await.unwrap();

    assert!(store.abort(&id("u1")).await.unwrap());
    assert!(!store.abort(&id("u1")).await.unwrap());
    assert!(matches!(
        store.append(&id("u1"), &data[..]).await,
        Err(StoreError::UploadUnknown)
    ));
    assert!(matches!(
        store.commit(&id("u1"), &sha256(&data)).await,
        Err(StoreError::UploadUnknown)
    ));
    assert_eq!(store.stat(&sha256(&data)).await.unwrap(), None);
    assert!(store.list_uploads().await.unwrap().is_empty());
}

pub async fn unknown_upload(store: &impl BlobStore) {
    assert!(matches!(
        store.append(&id("nope"), &b"x"[..]).await,
        Err(StoreError::UploadUnknown)
    ));
    assert!(matches!(
        store.commit(&id("nope"), &sha256(b"x")).await,
        Err(StoreError::UploadUnknown)
    ));
}

pub async fn start_upload_twice(store: &impl BlobStore) {
    store.start_upload(&id("u1")).await.unwrap();
    assert!(matches!(
        store.start_upload(&id("u1")).await,
        Err(StoreError::AlreadyExists)
    ));
}

/// A client that drops the connection mid-body: the append fails and nothing is
/// committed behind the caller's back.
pub async fn failed_append_returns_error(store: &impl BlobStore) {
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

    let data = fixture(5000, 12);
    store.start_upload(&id("u1")).await.unwrap();
    let result = store
        .append(&id("u1"), Cursor::new(data.clone()).chain(Broken))
        .await;

    assert!(matches!(result, Err(StoreError::Io(_))));
    assert_eq!(store.stat(&sha256(&data)).await.unwrap(), None);
    assert!(store.abort(&id("u1")).await.unwrap());
}

pub async fn delete_blob(store: &impl BlobStore) {
    let digest = upload(store, "u1", &[&fixture(64, 13)]).await;

    assert!(store.delete(&digest).await.unwrap());
    assert_eq!(store.stat(&digest).await.unwrap(), None);
    assert!(!store.delete(&digest).await.unwrap());
}

pub async fn list_paginates_in_digest_order(store: &impl BlobStore) {
    let mut expected = Vec::new();
    for seed in 0..5 {
        let data = fixture(100 + seed, 100 + seed as u64);
        let digest = upload(store, &format!("u{seed}"), &[&data]).await;
        expected.push((digest, data.len() as u64));
    }
    let data = fixture(77, 200);
    let digest = sha512(&data);
    store.start_upload(&id("u512")).await.unwrap();
    store.append(&id("u512"), &data[..]).await.unwrap();
    store.commit(&id("u512"), &digest).await.unwrap();
    expected.push((digest, 77));
    expected.sort();

    let mut listed = Vec::new();
    let mut after = None;
    loop {
        let page = store.list(after.as_ref(), 4).await.unwrap();
        assert!(page.len() <= 4);
        let Some(last) = page.last() else { break };
        after = Some(last.digest);
        listed.extend(page.into_iter().map(|e| (e.digest, e.size)));
    }

    assert_eq!(listed, expected);
}

pub async fn list_uploads_tracks_open_sessions(store: &impl BlobStore) {
    for u in ["a", "b", "c"] {
        store.start_upload(&id(u)).await.unwrap();
    }
    store.append(&id("a"), &b"data"[..]).await.unwrap();
    store.commit(&id("a"), &sha256(b"data")).await.unwrap();
    store.abort(&id("b")).await.unwrap();

    let open: Vec<String> = store
        .list_uploads()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.id.to_string())
        .collect();
    assert_eq!(open, ["c"]);
}
