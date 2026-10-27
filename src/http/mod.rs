mod oci;
mod probes;

use crate::registry::Uploads;
use crate::store::Storage;
use axum::{Router, routing::get};
use std::sync::Arc;

/// What every handler gets.
pub struct AppState {
    pub storage: Storage,
    pub uploads: Uploads,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/probe/live", get(probes::live))
        .route("/probe/ready", get(probes::ready))
        .nest_service("/v2", oci::service(state))
}
