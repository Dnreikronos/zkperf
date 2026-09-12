//! Deterministic starter files and conservative destination handling.

mod templates;

use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Create a starter manifest and its benchmark fixtures without prompting.
///
/// Only regular template files may be replaced when `force` is true. Existing
/// unrelated files are preserved. The manifest is written after its fixtures.
///
/// # Errors
/// Returns an I/O error on conflicts, unsafe template paths, or filesystem
/// failures. A write failure can leave a partially initialized directory.
pub fn initialize(directory: impl AsRef<Path>, force: bool) -> io::Result<()> {
    let directory = directory.as_ref();
    if directory.exists() && !directory.is_dir() {
        return Err(path_error(directory, "must be a directory"));
    }
    let benchmarks = directory.join("benchmarks");
    if let Some(metadata) = inspect(&benchmarks)? {
        if !metadata.is_dir() {
            return Err(path_error(
                &benchmarks,
                "must be a directory, not a symlink or file",
            ));
        }
    }
    for (relative, _) in templates::FILES {
        check_file(&directory.join(relative), force)?;
    }

    fs::create_dir_all(&benchmarks).map_err(|error| contextual(&benchmarks, &error))?;
    for (relative, contents) in templates::FILES {
        let path = directory.join(relative);
        // Recheck immediately before replacement; never truncate a symlink or
        // modify another name for the same inode through a hard link.
        if check_file(&path, force)? {
            fs::remove_file(&path).map_err(|error| contextual(&path, &error))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| contextual(&path, &error))?;
        file.write_all(contents)
            .map_err(|error| contextual(&path, &error))?;
    }
    Ok(())
}

fn check_file(path: &Path, force: bool) -> io::Result<bool> {
    let Some(metadata) = inspect(path)? else {
        return Ok(false);
    };
    if !force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already exists; use --force to replace template files",
                path.display()
            ),
        ));
    }
    if !metadata.is_file() {
        return Err(path_error(
            path,
            "refusing to replace a non-regular file or symlink",
        ));
    }
    Ok(true)
}

fn inspect(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(contextual(path, &error)),
    }
}

fn path_error(path: &Path, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{}: {message}", path.display()),
    )
}

fn contextual(path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}
