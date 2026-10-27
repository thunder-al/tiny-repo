//! Registry logic between the HTTP handlers and the store: upload sessions now,
//! manifest checks and pull rules later (THL-39, THL-41).

pub mod upload;

pub use upload::{UploadError, Uploads};
