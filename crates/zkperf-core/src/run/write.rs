use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::RunError;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// How many temporary names one write may try before giving up. A name is only
/// taken when another writer owns it, so a few retries are enough.
const TEMPORARY_ATTEMPTS: u32 = 16;

/// Creates a file that no earlier run or attempt can already own.
pub(super) fn create_new(path: &Path) -> Result<File, RunError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                RunError::AlreadyExists(path.to_path_buf())
            } else {
                RunError::io(path, error)
            }
        })
}

/// Publishes `contents` at `path` by renaming a completed temporary file, so a
/// reader observes either the previous file or the whole new one.
pub(super) fn atomic(path: &Path, contents: &[u8]) -> Result<(), RunError> {
    let (directory, temporary) = write_temporary(path, contents)?;
    if let Err(error) = fs::rename(&temporary, path) {
        drop(fs::remove_file(&temporary));
        return Err(RunError::io(path, error));
    }
    sync_directory(directory)
}

/// Writes a complete temporary file next to `path` and returns both.
fn write_temporary<'a>(path: &'a Path, contents: &[u8]) -> Result<(&'a Path, PathBuf), RunError> {
    let directory = path
        .parent()
        .ok_or_else(|| RunError::NotADirectory(path.to_path_buf()))?;
    let (temporary, mut file) = create_temporary(directory)?;

    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        // This call created the file, so removing it cannot drop evidence.
        drop(fs::remove_file(&temporary));
        return Err(RunError::io(&temporary, error));
    }
    Ok((directory, temporary))
}

/// Takes a temporary name, stepping over names another writer already holds.
fn create_temporary(directory: &Path) -> Result<(PathBuf, File), RunError> {
    let mut attempt = 1;
    loop {
        let candidate = directory.join(format!(
            ".zkperf-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        match create_new(&candidate) {
            Err(RunError::AlreadyExists(_)) if attempt < TEMPORARY_ATTEMPTS => attempt += 1,
            result => return result.map(|file| (candidate, file)),
        }
    }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{atomic, create_temporary};

    struct Directory(PathBuf);

    impl Directory {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("zkperf-write-{}-{name}", std::process::id()));
            drop(fs::remove_dir_all(&path));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    #[test]
    fn a_taken_temporary_name_is_stepped_over_instead_of_deleted() {
        let directory = Directory::new("temporary");
        let (taken, _handle) = create_temporary(&directory.0).unwrap();
        let sequence: u64 = taken
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.trim_end_matches(".tmp").rsplit('-').next())
            .and_then(|sequence| sequence.parse().ok())
            .unwrap();
        // The next write asks for this exact name.
        let planted = directory.0.join(format!(
            ".zkperf-{}-{}.tmp",
            std::process::id(),
            sequence + 1
        ));
        fs::write(&planted, b"evidence").unwrap();

        let published = directory.0.join("proof.bin");
        atomic(&published, b"proof").unwrap();

        assert_eq!(fs::read(&planted).unwrap(), b"evidence");
        assert_eq!(fs::read(&published).unwrap(), b"proof");
    }
}
