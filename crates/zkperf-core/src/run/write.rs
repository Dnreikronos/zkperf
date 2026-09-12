use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::RunError;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Creates a file that no earlier run or attempt can already own.
pub(super) fn create_new(path: &Path) -> Result<File, RunError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RunError::AlreadyExists(path.to_path_buf())
            } else {
                RunError::io(path, error)
            }
        })
}

/// Publishes `contents` at `path` by renaming a completed temporary file, so a
/// reader observes either the previous file or the whole new one.
pub(super) fn atomic(path: &Path, contents: &[u8]) -> Result<(), RunError> {
    let directory = path
        .parent()
        .ok_or_else(|| RunError::NotADirectory(path.to_path_buf()))?;
    let temporary = temporary_path(directory);

    let result = write_temporary(&temporary, contents)
        .and_then(|()| fs::rename(&temporary, path).map_err(|error| RunError::io(path, error)));
    if result.is_err() {
        // The published path is unchanged; drop the incomplete temporary file.
        drop(fs::remove_file(&temporary));
        return result;
    }
    sync_directory(directory)
}

fn write_temporary(path: &Path, contents: &[u8]) -> Result<(), RunError> {
    let mut file = create_new(path)?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| RunError::io(path, error))
}

fn temporary_path(directory: &Path) -> PathBuf {
    directory.join(format!(
        ".zkperf-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), RunError> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(|error| RunError::io(directory, error))
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), RunError> {
    // Windows has no portable directory handle to flush; the rename itself is
    // the durability boundary.
    Ok(())
}
