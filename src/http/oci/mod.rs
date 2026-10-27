//! The OCI distribution API under `/v2`. Handlers parse the request into protocol
//! types, call `registry`, and turn the result into the headers the spec asks for.

mod error;
mod path;
mod uploads;

#[cfg(test)]
mod tests;

pub use error::OciError;

use super::AppState;
use axum::http::{HeaderName, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router, extract::Request};
use std::sync::Arc;
use tower::Layer as _;
use tower::util::{MapRequest, MapRequestLayer};

/// Mounted at `/v2`; the repository-name folding runs before its routes match.
pub fn service(state: Arc<AppState>) -> MapRequest<Router, fn(Request) -> Request> {
    let routes = Router::new()
        .route("/", get(base))
        .route("/{repo}/blobs/uploads/", post(uploads::start))
        .route(
            "/{repo}/blobs/uploads/{id}",
            get(uploads::status)
                .patch(uploads::patch)
                .put(uploads::put)
                .delete(uploads::cancel),
        )
        .with_state(state);

    MapRequestLayer::new(path::fold_repo_name as fn(Request) -> Request).layer(routes)
}

/// `GET /v2/`: the client's check that this is a registry speaking API v2.
async fn base() -> impl IntoResponse {
    (
        [(
            HeaderName::from_static("docker-distribution-api-version"),
            HeaderValue::from_static("registry/2.0"),
        )],
        Json(serde_json::json!({})),
    )
}

/// The `Range` header value for an upload of `size` bytes: `0-<last byte>`, and
/// `0-0` for an empty one, as registries answer it.
fn range(size: u64) -> String {
    format!("0-{}", size.saturating_sub(1))
}
