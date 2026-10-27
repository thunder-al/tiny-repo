//! Local filesystem backend. Layout under the storage root:
//!
//! ```text
//! blobs/<alg>/<hex>           blobs
//! meta/index.json             all meta except manifest bodies
//! meta/manifests/<alg>/<hex>  manifest bodies, shared by repositories
//! ```
//!
//! Durability: every file that becomes visible is written in full, fsynced, then
//! renamed into place, and the directory is fsynced after the rename.

mod blob;
mod meta;

pub use blob::FsBlobStore;
pub use meta::FsMeta;

use std::io;
use std::path::Path;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;

/// Suffix of a file being written next to its final name. Never a valid hex digest
/// or index name, so readers skip it.
const TMP_SUFFIX: &str = ".tmp";

/// Writes `data` to `path` so that readers see the old file or the new one in full.
async fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data).await?;
    sync_parent(path).await
}

/// `write_atomic` without the final directory fsync: on error `path` still holds
/// the old content; on success it holds the new one, durable once the directory is
/// synced.
async fn replace(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(TMP_SUFFIX);

    let mut file = File::create(&tmp).await?;
    file.write_all(data).await?;
    file.sync_all().await?;
    drop(file);

    fs::rename(&tmp, path).await
}

/// Makes a rename or unlink in the file's directory durable.
async fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) => File::open(dir).await?.sync_all().await,
        None => Ok(()),
    }
}

/// Removes `*.tmp` files a crash left in `dir`.
async fn remove_tmp_files(dir: &Path) -> io::Result<()> {
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name().to_string_lossy().ends_with(TMP_SUFFIX) {
            super::staging::remove_if_exists(&entry.path()).await?;
        }
    }
    Ok(())
}
