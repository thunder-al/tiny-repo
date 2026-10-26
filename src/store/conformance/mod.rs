//! Behavior every backend must have. A backend runs the whole suite with
//! `blob_store_suite!(<expr>)` / `meta_suite!(<expr>)` in its test module; the
//! expression builds a fresh, empty store and may `.await`. Each case becomes its
//! own `#[tokio::test]`, so a failure names the case.

#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod blob;
pub mod meta;

use crate::oci::Digest;
use sha2::Digest as _;

macro_rules! cases {
    ($suite:ident, $make:expr; $($case:ident),* $(,)?) => {
        mod $suite {
            #[allow(unused_imports)]
            use super::*;

            $(
                #[tokio::test]
                async fn $case() {
                    let store = $make;
                    $crate::store::conformance::$suite::$case(&store).await;
                }
            )*
        }
    };
}

macro_rules! blob_store_suite {
    ($make:expr) => {
        $crate::store::conformance::cases!(blob, $make;
            missing_blob,
            upload_and_read,
            chunks_concatenate,
            large_upload,
            empty_blob,
            range_reads,
            sha512_blob,
            commit_existing_blob_keeps_it,
            concurrent_commits_of_same_blob,
            concurrent_uploads_are_isolated,
            abort_ends_upload,
            unknown_upload,
            start_upload_twice,
            failed_append_returns_error,
            delete_blob,
            list_paginates_in_digest_order,
            list_uploads_tracks_open_sessions,
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
