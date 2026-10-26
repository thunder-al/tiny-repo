use super::{fixture, sha256};
use crate::oci::{Digest, Reference, RepoName, Tag};
use crate::store::{
    AuthMeta, BlobRef, NewManifest, RegistryMeta, StoreError, TokenHash, TokenRecord, UnixTime,
    User,
};
use bytes::Bytes;

const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

fn repo(s: &str) -> RepoName {
    s.parse().unwrap()
}

fn tag(s: &str) -> Tag {
    s.parse().unwrap()
}

/// An image manifest with a config and `layers` layers, shaped like what the core
/// extracts from a real one. `seed` keeps manifests and their blobs distinct.
fn image(seed: u64, layers: usize) -> NewManifest {
    let blob = |n: u64| {
        let data = fixture(16, seed * 100 + n);
        BlobRef {
            digest: sha256(&data),
            size: 1000 * (n + 1),
        }
    };
    let blobs: Vec<BlobRef> = (0..=layers as u64).map(blob).collect();

    let body = format!(
        r#"{{"schemaVersion":2,"mediaType":"{OCI_MANIFEST}","config":{{"digest":"{}"}},"layers":[{}]}}"#,
        blobs[0].digest,
        blobs[1..]
            .iter()
            .map(|b| format!(r#"{{"digest":"{}","size":{}}}"#, b.digest, b.size))
            .collect::<Vec<_>>()
            .join(",")
    );

    NewManifest {
        digest: sha256(body.as_bytes()),
        media_type: OCI_MANIFEST.into(),
        body: Bytes::from(body),
        blobs,
        children: Vec::new(),
        subject: None,
        artifact_type: None,
    }
}

async fn tag_page(
    store: &impl RegistryMeta,
    r: &RepoName,
    after: Option<&str>,
    limit: usize,
) -> Vec<String> {
    store
        .list_tags(r, after.map(tag).as_ref(), limit)
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|t| t.tag.to_string())
        .collect()
}

async fn tag_names(store: &impl RegistryMeta, r: &RepoName) -> Vec<String> {
    tag_page(store, r, None, usize::MAX).await
}

async fn digest_of_tag(store: &impl RegistryMeta, r: &RepoName, t: &str) -> Option<Digest> {
    store
        .get_manifest(r, &Reference::Tag(tag(t)))
        .await
        .unwrap()
        .map(|m| m.digest)
}

pub async fn empty_store(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("library/nginx");
    let d = sha256(b"x");

    assert!(
        store
            .get_manifest(&r, &Reference::Digest(d))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_manifest(&r, &Reference::Tag(tag("latest")))
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.list_tags(&r, None, 10).await.unwrap().is_none());
    assert!(store.list_manifests(&r, None, 10).await.unwrap().is_none());
    assert!(store.get_repo(&r).await.unwrap().is_none());
    assert!(store.list_repos(None, 10).await.unwrap().is_empty());
    assert!(store.list_referrers(&r, &d).await.unwrap().is_empty());
    assert!(!store.repo_has_blob(&r, &d).await.unwrap());
    assert!(store.referenced_blobs().await.unwrap().is_empty());
    assert!(!store.set_repo_public(&r, true).await.unwrap());
    assert!(!store.delete_repo(&r).await.unwrap());
    assert!(!store.delete_tag(&r, &tag("latest")).await.unwrap());
    assert!(!store.delete_manifest(&r, &d).await.unwrap());
    assert!(store.list_users().await.unwrap().is_empty());
    assert!(store.list_tokens(None).await.unwrap().is_empty());
}

pub async fn put_and_get_by_tag_and_digest(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let m = image(1, 2);
    store.put_manifest(&r, &m, Some(&tag("v1"))).await.unwrap();

    for reference in [Reference::Tag(tag("v1")), Reference::Digest(m.digest)] {
        let got = store.get_manifest(&r, &reference).await.unwrap().unwrap();
        assert_eq!(got.digest, m.digest);
        assert_eq!(got.media_type, OCI_MANIFEST);
        assert_eq!(got.body, m.body, "exact bytes, no re-serialization");
    }
    assert_eq!(tag_names(store, &r).await, ["v1"]);
}

pub async fn put_without_tag(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let m = image(1, 1);
    store.put_manifest(&r, &m, None).await.unwrap();

    assert!(
        store
            .get_manifest(&r, &Reference::Digest(m.digest))
            .await
            .unwrap()
            .is_some()
    );
    assert!(tag_names(store, &r).await.is_empty());
    let summary = store.get_repo(&r).await.unwrap().unwrap();
    assert_eq!((summary.manifest_count, summary.tag_count), (1, 0));
}

pub async fn retag_moves_tag(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let (old, new) = (image(1, 1), image(2, 1));
    store
        .put_manifest(&r, &old, Some(&tag("latest")))
        .await
        .unwrap();
    store
        .put_manifest(&r, &new, Some(&tag("latest")))
        .await
        .unwrap();

    assert_eq!(digest_of_tag(store, &r, "latest").await, Some(new.digest));
    assert!(
        store
            .get_manifest(&r, &Reference::Digest(old.digest))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(tag_names(store, &r).await, ["latest"]);
}

pub async fn repeated_put_keeps_manifest(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let m = image(1, 1);
    store.put_manifest(&r, &m, Some(&tag("a"))).await.unwrap();
    store.put_manifest(&r, &m, Some(&tag("b"))).await.unwrap();

    assert_eq!(tag_names(store, &r).await, ["a", "b"]);
    assert_eq!(store.get_repo(&r).await.unwrap().unwrap().manifest_count, 1);
}

pub async fn repos_are_independent(store: &(impl RegistryMeta + AuthMeta)) {
    let (a, b) = (repo("team/a"), repo("team/b"));
    let (ma, mb) = (image(1, 1), image(2, 1));
    store
        .put_manifest(&a, &ma, Some(&tag("latest")))
        .await
        .unwrap();
    store
        .put_manifest(&b, &mb, Some(&tag("latest")))
        .await
        .unwrap();

    assert_eq!(digest_of_tag(store, &a, "latest").await, Some(ma.digest));
    assert_eq!(digest_of_tag(store, &b, "latest").await, Some(mb.digest));
    assert!(
        store
            .get_manifest(&a, &Reference::Digest(mb.digest))
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn tags_sorted_and_paginated(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let m = image(1, 1);
    for t in ["v10", "latest", "v2", "1.0", "V3", "_dev"] {
        store.put_manifest(&r, &m, Some(&tag(t))).await.unwrap();
    }

    // Lexical byte order, as the spec requires for tags/list.
    let all = ["1.0", "V3", "_dev", "latest", "v10", "v2"];
    assert_eq!(tag_names(store, &r).await, all);

    assert_eq!(tag_page(store, &r, None, 2).await, ["1.0", "V3"]);
    assert_eq!(
        tag_page(store, &r, Some("V3"), 3).await,
        ["_dev", "latest", "v10"]
    );
    assert_eq!(tag_page(store, &r, Some("v10"), 3).await, ["v2"]);
    assert!(tag_page(store, &r, Some("v2"), 3).await.is_empty());
    // `last` need not be an existing tag.
    assert_eq!(tag_page(store, &r, Some("m"), 10).await, ["v10", "v2"]);

    let entries = store.list_tags(&r, None, 1).await.unwrap().unwrap();
    assert_eq!(entries[0].digest, m.digest);
}

pub async fn repos_listed_with_summaries(store: &(impl RegistryMeta + AuthMeta)) {
    let before = UnixTime::now();
    let shared = image(1, 2);
    let mut other = image(2, 1);
    // A second image reusing the first one's base layer: counted once in the size.
    other.blobs.push(shared.blobs[1]);

    let r = repo("b/app");
    store
        .put_manifest(&r, &shared, Some(&tag("v1")))
        .await
        .unwrap();
    store
        .put_manifest(&r, &other, Some(&tag("v2")))
        .await
        .unwrap();
    for name in ["c", "a"] {
        store
            .put_manifest(&repo(name), &image(9, 0), None)
            .await
            .unwrap();
    }

    let names: Vec<String> = store
        .list_repos(None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name.to_string())
        .collect();
    assert_eq!(names, ["a", "b/app", "c"]);

    let page: Vec<String> = store
        .list_repos(Some(&repo("a")), 1)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name.to_string())
        .collect();
    assert_eq!(page, ["b/app"]);

    let s = store.get_repo(&r).await.unwrap().unwrap();
    let distinct: u64 = shared
        .blobs
        .iter()
        .chain(&other.blobs[..2])
        .map(|b| b.size)
        .sum();
    assert_eq!((s.tag_count, s.manifest_count, s.public), (2, 2, false));
    assert_eq!(s.size, distinct);
    assert!(s.updated >= before);
}

pub async fn set_repo_public(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    store.put_manifest(&r, &image(1, 1), None).await.unwrap();

    assert!(store.set_repo_public(&r, true).await.unwrap());
    assert!(store.get_repo(&r).await.unwrap().unwrap().public);
    assert!(store.set_repo_public(&r, false).await.unwrap());
    assert!(!store.get_repo(&r).await.unwrap().unwrap().public);
}

pub async fn delete_tag(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let m = image(1, 1);
    store.put_manifest(&r, &m, Some(&tag("v1"))).await.unwrap();

    assert!(store.delete_tag(&r, &tag("v1")).await.unwrap());
    assert!(!store.delete_tag(&r, &tag("v1")).await.unwrap());
    assert!(digest_of_tag(store, &r, "v1").await.is_none());
    assert!(
        store
            .get_manifest(&r, &Reference::Digest(m.digest))
            .await
            .unwrap()
            .is_some()
    );
}

pub async fn delete_manifest_drops_its_tags(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let (gone, kept) = (image(1, 1), image(2, 1));
    store
        .put_manifest(&r, &gone, Some(&tag("a")))
        .await
        .unwrap();
    store
        .put_manifest(&r, &gone, Some(&tag("b")))
        .await
        .unwrap();
    store
        .put_manifest(&r, &kept, Some(&tag("c")))
        .await
        .unwrap();

    assert!(store.delete_manifest(&r, &gone.digest).await.unwrap());
    assert!(!store.delete_manifest(&r, &gone.digest).await.unwrap());
    assert!(
        store
            .get_manifest(&r, &Reference::Digest(gone.digest))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(tag_names(store, &r).await, ["c"]);
    assert!(
        !store
            .repo_has_blob(&r, &gone.blobs[1].digest)
            .await
            .unwrap()
    );
}

pub async fn delete_repo(store: &(impl RegistryMeta + AuthMeta)) {
    let (r, other) = (repo("app"), repo("other"));
    let m = image(1, 1);
    store.put_manifest(&r, &m, Some(&tag("v1"))).await.unwrap();
    store
        .put_manifest(&other, &image(2, 1), None)
        .await
        .unwrap();

    assert!(store.delete_repo(&r).await.unwrap());
    assert!(store.get_repo(&r).await.unwrap().is_none());
    assert!(store.list_tags(&r, None, 10).await.unwrap().is_none());
    assert!(
        store
            .get_manifest(&r, &Reference::Digest(m.digest))
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_repo(&other).await.unwrap().is_some());

    // Pushing again recreates it from scratch.
    store.put_manifest(&r, &image(3, 1), None).await.unwrap();
    assert!(tag_names(store, &r).await.is_empty());
}

pub async fn manifests_listed_with_tags(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let (tagged, untagged) = (image(1, 1), image(2, 1));
    store
        .put_manifest(&r, &tagged, Some(&tag("v2")))
        .await
        .unwrap();
    store
        .put_manifest(&r, &tagged, Some(&tag("v1")))
        .await
        .unwrap();
    store.put_manifest(&r, &untagged, None).await.unwrap();

    let list = store.list_manifests(&r, None, 10).await.unwrap().unwrap();
    let mut expected = [tagged.digest, untagged.digest];
    expected.sort();
    assert_eq!(list.iter().map(|m| m.digest).collect::<Vec<_>>(), expected);

    let t = list.iter().find(|m| m.digest == tagged.digest).unwrap();
    assert_eq!(t.tags, [tag("v1"), tag("v2")]);
    assert_eq!(t.size, tagged.body.len() as u64);
    assert_eq!(t.media_type, OCI_MANIFEST);
    let u = list.iter().find(|m| m.digest == untagged.digest).unwrap();
    assert!(u.tags.is_empty());

    let page = store
        .list_manifests(&r, Some(&expected[0]), 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].digest, expected[1]);
}

pub async fn referrers_by_subject(store: &(impl RegistryMeta + AuthMeta)) {
    let (r, other) = (repo("app"), repo("other"));
    let subject = image(1, 1);
    let mut signature = image(2, 0);
    signature.subject = Some(subject.digest);
    signature.artifact_type = Some("application/vnd.dev.cosign.artifact.sig.v1+json".into());

    store
        .put_manifest(&r, &subject, Some(&tag("v1")))
        .await
        .unwrap();
    store.put_manifest(&r, &signature, None).await.unwrap();
    store.put_manifest(&other, &signature, None).await.unwrap();

    let refs = store.list_referrers(&r, &subject.digest).await.unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].digest, signature.digest);
    assert_eq!(refs[0].artifact_type, signature.artifact_type);
    assert!(
        store
            .list_referrers(&r, &signature.digest)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn blob_index_follows_manifests(store: &(impl RegistryMeta + AuthMeta)) {
    let (a, b) = (repo("a"), repo("b"));
    let (ma, mb) = (image(1, 2), image(2, 1));
    store.put_manifest(&a, &ma, None).await.unwrap();
    store.put_manifest(&b, &mb, None).await.unwrap();
    store.put_manifest(&b, &ma, None).await.unwrap();

    for blob in &ma.blobs {
        assert!(store.repo_has_blob(&a, &blob.digest).await.unwrap());
    }
    assert!(!store.repo_has_blob(&a, &mb.blobs[1].digest).await.unwrap());
    // Manifest digests are not blobs.
    assert!(!store.repo_has_blob(&a, &ma.digest).await.unwrap());

    let mut all: Vec<Digest> = ma.blobs.iter().chain(&mb.blobs).map(|b| b.digest).collect();
    all.sort();
    all.dedup();
    assert_eq!(store.referenced_blobs().await.unwrap(), all);

    store.delete_manifest(&b, &ma.digest).await.unwrap();
    assert!(store.repo_has_blob(&a, &ma.blobs[1].digest).await.unwrap());
    assert!(!store.repo_has_blob(&b, &ma.blobs[1].digest).await.unwrap());
    store.delete_repo(&a).await.unwrap();
    let left = store.referenced_blobs().await.unwrap();
    assert!(!left.contains(&ma.blobs[1].digest));
    assert!(left.contains(&mb.blobs[1].digest));
}

pub async fn concurrent_puts_to_one_repo(store: &(impl RegistryMeta + AuthMeta)) {
    let r = repo("app");
    let (m1, m2, m3) = (image(1, 1), image(2, 1), image(3, 1));
    let (ta, tb, tc) = (tag("a"), tag("b"), tag("c"));
    let (a, b, c) = tokio::join!(
        store.put_manifest(&r, &m1, Some(&ta)),
        store.put_manifest(&r, &m2, Some(&tb)),
        store.put_manifest(&r, &m3, Some(&tc)),
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();

    assert_eq!(tag_names(store, &r).await, ["a", "b", "c"]);
    assert_eq!(store.get_repo(&r).await.unwrap().unwrap().manifest_count, 3);
}

fn user(name: &str, admin: bool) -> User {
    User {
        name: name.parse().unwrap(),
        admin,
        created: UnixTime(1_700_000_000),
    }
}

fn token(id: &str, owner: &str, seed: u8) -> TokenRecord {
    TokenRecord {
        id: id.parse().unwrap(),
        hash: TokenHash([seed; 32]),
        user: owner.parse().unwrap(),
        description: format!("ci token {id}"),
        scopes: vec!["repository:team/*:pull,push".into()],
        expires: Some(UnixTime(1_800_000_000)),
        created: UnixTime(1_700_000_000),
        last_used: None,
    }
}

pub async fn users_crud(store: &(impl RegistryMeta + AuthMeta)) {
    store.create_user(&user("bob", false)).await.unwrap();
    store.create_user(&user("alice", true)).await.unwrap();
    assert!(matches!(
        store.create_user(&user("bob", true)).await,
        Err(StoreError::AlreadyExists)
    ));

    let names: Vec<String> = store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.name.to_string())
        .collect();
    assert_eq!(names, ["alice", "bob"]);

    let bob = "bob".parse().unwrap();
    assert_eq!(
        store.get_user(&bob).await.unwrap(),
        Some(user("bob", false))
    );
    assert!(store.update_user(&user("bob", true)).await.unwrap());
    assert!(store.get_user(&bob).await.unwrap().unwrap().admin);
    assert!(!store.update_user(&user("carol", true)).await.unwrap());

    assert!(store.delete_user(&bob).await.unwrap());
    assert!(!store.delete_user(&bob).await.unwrap());
    assert!(store.get_user(&bob).await.unwrap().is_none());
}

pub async fn tokens_crud(store: &(impl RegistryMeta + AuthMeta)) {
    store.create_user(&user("alice", true)).await.unwrap();
    store.create_user(&user("bob", false)).await.unwrap();
    let (t1, t2, t3) = (
        token("aaa1", "alice", 1),
        token("bbb2", "bob", 2),
        token("aaa0", "alice", 3),
    );
    for t in [&t1, &t2, &t3] {
        store.create_token(t).await.unwrap();
    }

    assert_eq!(
        store.get_token_by_hash(&t2.hash).await.unwrap(),
        Some(t2.clone())
    );
    assert!(
        store
            .get_token_by_hash(&TokenHash([9; 32]))
            .await
            .unwrap()
            .is_none()
    );

    let ids = |list: Vec<TokenRecord>| {
        list.into_iter()
            .map(|t| t.id.to_string())
            .collect::<Vec<_>>()
    };
    let alice = "alice".parse().unwrap();
    assert_eq!(
        ids(store.list_tokens(Some(&alice)).await.unwrap()),
        ["aaa0", "aaa1"]
    );
    assert_eq!(
        ids(store.list_tokens(None).await.unwrap()),
        ["aaa0", "aaa1", "bbb2"]
    );

    assert!(
        store
            .set_token_last_used(&t1.id, UnixTime(1_750_000_000))
            .await
            .unwrap()
    );
    let used = store.get_token_by_hash(&t1.hash).await.unwrap().unwrap();
    assert_eq!(used.last_used, Some(UnixTime(1_750_000_000)));

    assert!(store.revoke_token(&t1.id).await.unwrap());
    assert!(!store.revoke_token(&t1.id).await.unwrap());
    assert!(store.get_token_by_hash(&t1.hash).await.unwrap().is_none());
    assert!(
        !store
            .set_token_last_used(&t1.id, UnixTime(1))
            .await
            .unwrap()
    );
}

pub async fn token_requires_user_and_unique_id_and_hash(store: &(impl RegistryMeta + AuthMeta)) {
    assert!(matches!(
        store.create_token(&token("aaa1", "ghost", 1)).await,
        Err(StoreError::UserUnknown)
    ));

    store.create_user(&user("alice", true)).await.unwrap();
    store
        .create_token(&token("aaa1", "alice", 1))
        .await
        .unwrap();
    assert!(matches!(
        store.create_token(&token("aaa1", "alice", 2)).await,
        Err(StoreError::AlreadyExists)
    ));
    assert!(matches!(
        store.create_token(&token("ccc3", "alice", 1)).await,
        Err(StoreError::AlreadyExists)
    ));
    assert_eq!(store.list_tokens(None).await.unwrap().len(), 1);
}

pub async fn delete_user_drops_tokens(store: &(impl RegistryMeta + AuthMeta)) {
    store.create_user(&user("alice", true)).await.unwrap();
    store.create_user(&user("bob", false)).await.unwrap();
    let (ta, tb) = (token("aaa1", "alice", 1), token("bbb2", "bob", 2));
    store.create_token(&ta).await.unwrap();
    store.create_token(&tb).await.unwrap();

    store.delete_user(&"bob".parse().unwrap()).await.unwrap();

    assert!(store.get_token_by_hash(&tb.hash).await.unwrap().is_none());
    assert!(store.get_token_by_hash(&ta.hash).await.unwrap().is_some());
}
