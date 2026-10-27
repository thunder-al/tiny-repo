//! Errors of the OCI distribution API: one place for status codes and the
//! `{"errors":[{"code","message","detail"}]}` body.

use crate::registry::UploadError;
use crate::store::StoreError;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum OciError {
    #[error("blob upload unknown to registry")]
    BlobUploadUnknown,
    #[error("blob upload invalid: {0}")]
    BlobUploadInvalid(String),
    /// A chunk that does not continue the upload; `size` is what the server has.
    #[error("chunk out of order, upload has {size} bytes")]
    RangeInvalid { size: u64 },
    #[error("provided digest did not match uploaded content: {0}")]
    DigestInvalid(String),
    #[error("invalid repository name: {0}")]
    NameInvalid(String),
    #[error("blob too large: {0}")]
    SizeInvalid(String),
    #[error("internal error")]
    Internal,
}

impl OciError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BlobUploadUnknown => (StatusCode::NOT_FOUND, "BLOB_UPLOAD_UNKNOWN"),
            Self::BlobUploadInvalid(_) => (StatusCode::BAD_REQUEST, "BLOB_UPLOAD_INVALID"),
            Self::RangeInvalid { .. } => (StatusCode::RANGE_NOT_SATISFIABLE, "BLOB_UPLOAD_INVALID"),
            Self::DigestInvalid(_) => (StatusCode::BAD_REQUEST, "DIGEST_INVALID"),
            Self::NameInvalid(_) => (StatusCode::BAD_REQUEST, "NAME_INVALID"),
            Self::SizeInvalid(_) => (StatusCode::PAYLOAD_TOO_LARGE, "SIZE_INVALID"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "UNKNOWN"),
        }
    }
}

impl IntoResponse for OciError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let body = json!({ "errors": [{ "code": code, "message": self.to_string() }] });
        let mut response = (status, Json(body)).into_response();

        if let Self::RangeInvalid { size } = self
            && let Ok(range) = HeaderValue::from_str(&super::range(size))
        {
            response.headers_mut().insert(header::RANGE, range);
        }
        response
    }
}

impl From<StoreError> for OciError {
    fn from(err: StoreError) -> Self {
        tracing::error!(%err, "storage error");
        Self::Internal
    }
}

impl From<UploadError> for OciError {
    fn from(err: UploadError) -> Self {
        match err {
            UploadError::Unknown => Self::BlobUploadUnknown,
            UploadError::OutOfOrder { size, .. } => Self::RangeInvalid { size },
            UploadError::TooLarge { .. } => Self::SizeInvalid(err.to_string()),
            UploadError::Body(_) => Self::BlobUploadInvalid(err.to_string()),
            UploadError::DigestMismatch(_) => Self::DigestInvalid(err.to_string()),
            UploadError::Store(e) => e.into(),
        }
    }
}
