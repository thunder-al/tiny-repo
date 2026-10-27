//! Blob upload routes:
//!
//! ```text
//! POST   /v2/<name>/blobs/uploads/                     start (or mount, or upload in one go)
//! PATCH  /v2/<name>/blobs/uploads/<id>                 append a chunk
//! PUT    /v2/<name>/blobs/uploads/<id>?digest=<d>      last chunk, check, store
//! GET    /v2/<name>/blobs/uploads/<id>                 progress
//! DELETE /v2/<name>/blobs/uploads/<id>                 cancel
//! ```

use super::{OciError, range};
use crate::http::AppState;
use crate::oci::{Digest, RepoName, UploadId};
use crate::registry::upload::mountable;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt as _;
use serde::Deserialize;
use std::io;
use std::sync::Arc;
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;

const UPLOAD_UUID: HeaderName = HeaderName::from_static("docker-upload-uuid");
const CONTENT_DIGEST: HeaderName = HeaderName::from_static("docker-content-digest");

#[derive(Deserialize)]
pub struct StartQuery {
    digest: Option<String>,
    mount: Option<String>,
    from: Option<String>,
}

#[derive(Deserialize)]
pub struct PutQuery {
    digest: Option<String>,
}

type St = State<Arc<AppState>>;

pub async fn start(
    State(state): St,
    Path(repo): Path<String>,
    Query(q): Query<StartQuery>,
    body: Body,
) -> Result<Response, OciError> {
    let repo = parse_repo(&repo)?;

    // Cross-repository mount: without `from`, or when the blob is not there, the
    // spec says to fall back to a regular upload.
    if let (Some(mount), Some(from)) = (&q.mount, &q.from) {
        let digest = parse_digest(mount)?;
        if mountable(&state.storage, &parse_repo(from)?, &digest).await? {
            return Ok(created(&repo, &digest));
        }
    }

    let id = state.uploads.start(&state.storage.staging, &repo).await?;

    match &q.digest {
        Some(digest) => {
            let digest = parse_digest(digest)?;
            state
                .uploads
                .finish(&state.storage, &repo, &id, &digest, reader(body))
                .await?;
            Ok(created(&repo, &digest))
        }
        None => Ok(in_progress(StatusCode::ACCEPTED, &repo, &id, 0)),
    }
}

pub async fn patch(
    State(state): St,
    Path((repo, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, OciError> {
    let (repo, id) = (parse_repo(&repo)?, parse_id(&id)?);
    let start = content_range_start(&headers)?;

    let size = state
        .uploads
        .append(&state.storage.staging, &repo, &id, start, reader(body))
        .await?;
    Ok(in_progress(StatusCode::ACCEPTED, &repo, &id, size))
}

pub async fn put(
    State(state): St,
    Path((repo, id)): Path<(String, String)>,
    Query(q): Query<PutQuery>,
    body: Body,
) -> Result<Response, OciError> {
    let (repo, id) = (parse_repo(&repo)?, parse_id(&id)?);
    let digest = q
        .digest
        .as_deref()
        .ok_or_else(|| OciError::DigestInvalid("the digest query parameter is required".into()))
        .and_then(parse_digest)?;

    state
        .uploads
        .finish(&state.storage, &repo, &id, &digest, reader(body))
        .await?;
    Ok(created(&repo, &digest))
}

pub async fn status(
    State(state): St,
    Path((repo, id)): Path<(String, String)>,
) -> Result<Response, OciError> {
    let (repo, id) = (parse_repo(&repo)?, parse_id(&id)?);
    let size = state.uploads.size(&repo, &id).await?;
    Ok(in_progress(StatusCode::NO_CONTENT, &repo, &id, size))
}

pub async fn cancel(
    State(state): St,
    Path((repo, id)): Path<(String, String)>,
) -> Result<Response, OciError> {
    let (repo, id) = (parse_repo(&repo)?, parse_id(&id)?);
    state
        .uploads
        .cancel(&state.storage.staging, &repo, &id)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn in_progress(status: StatusCode, repo: &RepoName, id: &UploadId, size: u64) -> Response {
    (
        status,
        [
            (header::LOCATION, format!("/v2/{repo}/blobs/uploads/{id}")),
            (header::RANGE, range(size)),
            (UPLOAD_UUID, id.to_string()),
        ],
    )
        .into_response()
}

fn created(repo: &RepoName, digest: &Digest) -> Response {
    (
        StatusCode::CREATED,
        [
            (header::LOCATION, format!("/v2/{repo}/blobs/{digest}")),
            (CONTENT_DIGEST, digest.to_string()),
        ],
    )
        .into_response()
}

/// The request body as a byte stream. A body error is the client going away, not
/// a server fault; `Uploads` tells the two apart.
fn reader(body: Body) -> impl AsyncRead + Send + Unpin {
    StreamReader::new(body.map_err(io::Error::other).into_data_stream())
}

/// Start of `Content-Range: <start>-<end>` (no `bytes=` prefix in this API).
fn content_range_start(headers: &HeaderMap) -> Result<Option<u64>, OciError> {
    let Some(value) = headers.get(header::CONTENT_RANGE) else {
        return Ok(None);
    };
    let invalid = || OciError::BlobUploadInvalid("malformed Content-Range".into());

    let value = value.to_str().map_err(|_| invalid())?;
    let (start, end) = value.split_once('-').ok_or_else(invalid)?;
    let (start, end) = (
        start.trim().parse::<u64>().map_err(|_| invalid())?,
        end.trim().parse::<u64>().map_err(|_| invalid())?,
    );
    if end < start {
        return Err(invalid());
    }
    Ok(Some(start))
}

fn parse_repo(s: &str) -> Result<RepoName, OciError> {
    s.parse().map_err(|_| OciError::NameInvalid(s.to_owned()))
}

/// A malformed id cannot name a session.
fn parse_id(s: &str) -> Result<UploadId, OciError> {
    s.parse().map_err(|_| OciError::BlobUploadUnknown)
}

fn parse_digest(s: &str) -> Result<Digest, OciError> {
    s.parse()
        .map_err(|_| OciError::DigestInvalid(format!("unsupported or malformed digest {s:?}")))
}
