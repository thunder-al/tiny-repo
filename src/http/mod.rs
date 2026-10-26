mod probes;

use axum::{Router, routing::get};

pub fn router() -> Router {
    Router::new()
        .route("/probe/live", get(probes::live))
        .route("/probe/ready", get(probes::ready))
}
