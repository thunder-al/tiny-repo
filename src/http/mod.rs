mod probes;

use crate::store::Storage;
use axum::{Router, routing::get};
use std::sync::Arc;

/// What every handler gets.
pub struct AppState {
    #[allow(dead_code)] // read by the OCI routes (THL-39, THL-41, THL-53)
    pub storage: Storage,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/probe/live", get(probes::live))
        .route("/probe/ready", get(probes::ready))
        .with_state(state)
}
