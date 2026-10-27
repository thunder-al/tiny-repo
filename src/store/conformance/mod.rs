//! Behavior every backend must have. A backend runs the whole suite with
//! `blob_store_suite!(<expr>)` / `meta_suite!(<expr>)` in its test module; the
//! expression builds a fresh, empty store wrapped in a fixture (`Blobs` for blobs,
//! `Held` for meta) and may `.await`. Each case becomes its own
//! `#[tokio::test]`, so a failure names the case.

#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod blob;
pub mod meta;

pub use blob::Blobs;

use crate::oci::Digest;
use sha2::Digest as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! cases {
    ($suite:ident, $make:expr; $($case:ident),* $(,)?) => {
        mod $suite {
            #[allow(unused_imports)]
            use super::*;

            $(
                #[tokio::test]
                async fn $case() {
                    let fixture = $make;
                    $crate::store::conformance::$suite::$case(fixture.as_ref()).await;
                }
            )*
        }
    };
}

macro_rules! blob_store_suite {
    ($make:expr) => {
        $crate::store::conformance::cases!(blob, $make;
            missing_blob,
            put_and_read,
            large_blob,
            empty_blob,
            range_reads,
            sha512_blob,
            put_existing_blob_keeps_it,
            concurrent_puts_of_same_blob,
            delete_blob,
            list_paginates_in_digest_order,
        );
    };
}

macro_rules! meta_suite {
    ($make:expr) => {
        $crate::store::conformance::cases!(meta, $make;
            empty_store,
            put_and_get_by_tag_and_digest,
            put_without_tag,
            retag_moves_tag,
            repeated_put_keeps_manifest,
            repos_are_independent,
            tags_sorted_and_paginated,
            repos_listed_with_summaries,
            set_repo_public,
            delete_tag,
            delete_manifest_drops_its_tags,
            delete_repo,
            manifests_listed_with_tags,
            referrers_by_subject,
            blob_index_follows_manifests,
            concurrent_puts_to_one_repo,
            users_crud,
            tokens_crud,
            token_requires_user_and_unique_id_and_hash,
            delete_user_drops_tokens,
        );
    };
}

pub(crate) use {blob_store_suite, cases, meta_suite};

/// A store with the temp dir it lives in, so the dir outlives it.
pub struct Held<S> {
    store: S,
    _dir: Option<TempDir>,
}

impl<S> Held<S> {
    pub fn new(store: S) -> Self {
        Self { store, _dir: None }
    }

    pub fn in_dir(store: S, dir: TempDir) -> Self {
        Self {
            store,
            _dir: Some(dir),
        }
    }
}

impl<S> AsRef<S> for Held<S> {
    fn as_ref(&self) -> &S {
        &self.store
    }
}

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tiny-repo-test-{}-{n}", std::process::id()));
        // Left over from an earlier run that had the same pid.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn sha256(data: &[u8]) -> Digest {
    Digest::Sha256(sha2::Sha256::digest(data).into())
}

pub fn sha512(data: &[u8]) -> Digest {
    Digest::Sha512(sha2::Sha512::digest(data).into())
}

/// Deterministic pseudo-random bytes (xorshift), so content differs between seeds
/// and does not compress into something a backend could special-case.
pub fn fixture(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[0]
        })
        .collect()
}
