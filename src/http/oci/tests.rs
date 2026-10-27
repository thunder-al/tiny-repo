//! The upload routes end to end over a real fs store, one test per way clients
//! upload (THL-35) plus the error paths.

#![allow(clippy::unwrap_used)]

use crate::http::{AppState, router};
use crate::oci::{Digest, RepoName};
use crate::registry::Uploads;
use crate::store::conformance::{TempDir, fixture, sha256, sha512};
use crate::store::{BlobRef, BlobStore, NewManifest, RegistryMeta, open};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt as _;
use tower::ServiceExt as _;

struct App {
    router: Router,
    state: Arc<AppState>,
    _dir: TempDir,
}

async fn app_with_limit(max_blob_size: Option<u64>) -> App {
    let dir = TempDir::new();
    let config = crate::config::Config {
        storage: crate::config::StorageConfig::Fs {
            root: dir.path().to_owned(),
        },
        ..Default::default()
    };
    let storage = open(&config.storage, &config.meta, &config.uploads)
        .await
        .unwrap();
    let state = Arc::new(AppState {
        storage,
        uploads: Uploads::new(max_blob_size, Duration::from_secs(3600)),
    });
    App {
        router: router(Arc::clone(&state)),
        state,
        _dir: dir,
    }
}

async fn app() -> App {
    app_with_limit(None).await
}

impl App {
    async fn send(&self, method: Method, uri: &str, body: Body) -> Response<Body> {
        self.request(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(body)
                .unwrap(),
        )
        .await
    }

    async fn request(&self, req: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(req).await.unwrap()
    }

    /// POST a new session and return its `Location`.
    async fn start(&self, repo: &str) -> String {
        let resp = self
            .send(
                Method::POST,
                &format!("/v2/{repo}/blobs/uploads/"),
                Body::empty(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(header(&resp, "range"), "0-0");
        header(&resp, "location")
    }

    async fn stored(&self, digest: &Digest) -> Vec<u8> {
        let mut out = Vec::new();
        let mut body = self
            .state
            .storage
            .blobs
            .get(digest, None)
            .await
            .unwrap()
            .unwrap()
            .body;
        body.read_to_end(&mut out).await.unwrap();
        out
    }

    async fn staged_count(&self) -> usize {
        self.state.storage.staging.list().await.unwrap().len()
    }
}

fn header(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get(name)
        .unwrap_or_else(|| panic!("no {name} header"))
        .to_str()
        .unwrap()
        .to_owned()
}

async fn error_code(resp: Response<Body>) -> String {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json["errors"][0]["code"].as_str().unwrap().to_owned()
}

/// A body without a length, read in pieces: what `Transfer-Encoding: chunked`
/// looks like to the handler.
fn chunked(data: &[u8]) -> Body {
    let reader = std::io::Cursor::new(data.to_vec());
    Body::from_stream(tokio_util::io::ReaderStream::new(reader))
}

fn digest_uri(location: &str, digest: &Digest) -> String {
    format!("{location}?digest={digest}")
}

#[tokio::test]
async fn base_answers_api_v2() {
    let app = app().await;
    let resp = app.send(Method::GET, "/v2/", Body::empty()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "docker-distribution-api-version"),
        "registry/2.0"
    );
}

/// docker classic, skopeo, podman: one PATCH with the whole layer and no length,
/// then a PUT without a body.
#[tokio::test]
async fn chunked_patch_then_empty_put() {
    let app = app().await;
    let data = fixture(100_000, 1);
    let digest = sha256(&data);
    let location = app.start("team/app").await;
    assert!(
        location.starts_with("/v2/team/app/blobs/uploads/"),
        "{location}"
    );

    let resp = app.send(Method::PATCH, &location, chunked(&data)).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(header(&resp, "range"), "0-99999");
    assert_eq!(header(&resp, "location"), location);

    let resp = app
        .send(Method::PUT, &digest_uri(&location, &digest), Body::empty())
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        header(&resp, "location"),
        format!("/v2/team/app/blobs/{digest}")
    );
    assert_eq!(header(&resp, "docker-content-digest"), digest.to_string());
    assert_eq!(app.stored(&digest).await, data);
    assert_eq!(app.staged_count().await, 0);
}

/// crane: PATCH with `Content-Length`, then a PUT without a body; here in two
/// chunks with `Content-Range`.
#[tokio::test]
async fn ranged_patches_then_empty_put() {
    let app = app().await;
    let data = fixture(3000, 2);
    let digest = sha256(&data);
    let location = app.start("app").await;

    for (start, end) in [(0, 1000), (1000, 3000)] {
        let req = Request::builder()
            .method(Method::PATCH)
            .uri(&location)
            .header(header::CONTENT_LENGTH, end - start)
            .header(header::CONTENT_RANGE, format!("{start}-{}", end - 1))
            .body(Body::from(data[start..end].to_vec()))
            .unwrap();
        let resp = app.request(req).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(header(&resp, "range"), format!("0-{}", end - 1));
    }

    let resp = app
        .send(Method::PUT, &digest_uri(&location, &digest), Body::empty())
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(app.stored(&digest).await, data);
}

/// containerd, buildx, regctl: an empty POST, then the whole blob in the PUT.
#[tokio::test]
async fn post_then_put_with_body() {
    let app = app().await;
    let data = fixture(5000, 3);
    let digest = sha256(&data);
    let location = app.start("team/sub/app").await;

    let resp = app
        .send(
            Method::PUT,
            &digest_uri(&location, &digest),
            Body::from(data.clone()),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(app.stored(&digest).await, data);
}

/// The single-request upload of the spec: `POST ?digest=` with the blob.
#[tokio::test]
async fn post_with_digest_and_body() {
    let app = app().await;
    let data = fixture(2000, 4);
    let digest = sha256(&data);

    let resp = app
        .send(
            Method::POST,
            &format!("/v2/team/app/blobs/uploads/?digest={digest}"),
            Body::from(data.clone()),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        header(&resp, "location"),
        format!("/v2/team/app/blobs/{digest}")
    );
    assert_eq!(app.stored(&digest).await, data);
}

#[tokio::test]
async fn sha512_digest() {
    let app = app().await;
    let data = fixture(2000, 5);
    let digest = sha512(&data);
    let location = app.start("app").await;

    let resp = app
        .send(
            Method::PUT,
            &digest_uri(&location, &digest),
            Body::from(data.clone()),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(app.stored(&digest).await, data);
}

#[tokio::test]
async fn existing_blob_is_not_stored_twice() {
    let app = app().await;
    let data = fixture(2000, 6);
    let digest = sha256(&data);

    for _ in 0..2 {
        let location = app.start("app").await;
        let resp = app
            .send(
                Method::PUT,
                &digest_uri(&location, &digest),
                Body::from(data.clone()),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    assert_eq!(
        app.state.storage.blobs.list(None, 10).await.unwrap().len(),
        1
    );
    assert_eq!(app.staged_count().await, 0);
}

#[tokio::test]
async fn wrong_digest_ends_session() {
    let app = app().await;
    let location = app.start("app").await;
    let wrong = sha256(b"something else");

    let resp = app
        .send(
            Method::PUT,
            &digest_uri(&location, &wrong),
            Body::from(fixture(100, 7)),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(resp).await, "DIGEST_INVALID");

    let resp = app.send(Method::GET, &location, Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(app.staged_count().await, 0);
    assert!(
        app.state
            .storage
            .blobs
            .stat(&wrong)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn put_without_digest_is_rejected() {
    let app = app().await;
    let location = app.start("app").await;

    let resp = app.send(Method::PUT, &location, Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(resp).await, "DIGEST_INVALID");
}

#[tokio::test]
async fn out_of_order_chunk_gets_416_with_current_range() {
    let app = app().await;
    let location = app.start("app").await;
    app.send(Method::PATCH, &location, Body::from(fixture(100, 8)))
        .await;

    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .header(header::CONTENT_RANGE, "50-149")
        .body(Body::from(fixture(100, 9)))
        .unwrap();
    let resp = app.request(req).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(header(&resp, "range"), "0-99");

    // The session survives: the client can query it and continue.
    let resp = app.send(Method::GET, &location, Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&resp, "range"), "0-99");
}

#[tokio::test]
async fn blob_over_limit_gets_413() {
    let app = app_with_limit(Some(10_000)).await;
    let location = app.start("app").await;

    let resp = app
        .send(Method::PATCH, &location, chunked(&fixture(20_000, 10)))
        .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(resp).await, "SIZE_INVALID");
    assert_eq!(app.staged_count().await, 0);
}

#[tokio::test]
async fn session_of_another_repo_is_unknown() {
    let app = app().await;
    let location = app.start("team/a").await;
    let other = location.replace("/team/a/", "/team/b/");

    let resp = app.send(Method::PATCH, &other, Body::from(&b"x"[..])).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(resp).await, "BLOB_UPLOAD_UNKNOWN");
}

#[tokio::test]
async fn delete_cancels_session() {
    let app = app().await;
    let location = app.start("app").await;
    app.send(Method::PATCH, &location, Body::from(fixture(100, 11)))
        .await;

    let resp = app.send(Method::DELETE, &location, Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .send(Method::PATCH, &location, Body::from(&b"x"[..]))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(app.staged_count().await, 0);
}

#[tokio::test]
async fn mount_from_repo_that_has_the_blob() {
    let app = app().await;
    let data = fixture(1000, 12);
    let digest = sha256(&data);
    let location = app.start("team/base").await;
    app.send(
        Method::PUT,
        &digest_uri(&location, &digest),
        Body::from(data),
    )
    .await;

    // The blob belongs to team/base once a manifest there points at it.
    let manifest = Bytes::from_static(b"{}");
    let base: RepoName = "team/base".parse().unwrap();
    app.state
        .storage
        .meta
        .put_manifest(
            &base,
            &NewManifest {
                digest: sha256(&manifest),
                media_type: "application/vnd.oci.image.manifest.v1+json".into(),
                body: manifest,
                blobs: vec![BlobRef { digest, size: 1000 }],
                children: Vec::new(),
                subject: None,
                artifact_type: None,
            },
            None,
        )
        .await
        .unwrap();

    let mount = |from: &str| format!("/v2/team/app/blobs/uploads/?mount={digest}&from={from}");
    let resp = app
        .send(Method::POST, &mount("team/base"), Body::empty())
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        header(&resp, "location"),
        format!("/v2/team/app/blobs/{digest}")
    );

    // Not in `from`: a regular upload session instead.
    let resp = app
        .send(Method::POST, &mount("team/other"), Body::empty())
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn invalid_name_and_unknown_paths() {
    let app = app().await;

    let resp = app
        .send(Method::POST, "/v2/Bad/blobs/uploads/", Body::empty())
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(resp).await, "NAME_INVALID");

    for path in ["/v2/a/b/nothing", "/v2/Team/App/blobs/uploads/"] {
        let resp = app.send(Method::POST, path, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
