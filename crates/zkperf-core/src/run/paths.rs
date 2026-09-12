use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::RunError;

const MAX_SEGMENT_BYTES: usize = 255;

/// Windows opens these names as devices regardless of the directory they
/// appear in, so they are rejected on every platform for one portable layout.
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Splits a run-relative path into segments that cannot leave the run
/// directory, address a device, or need URI or shell escaping.
pub(super) fn segments(relative: &str) -> Result<Vec<&str>, RunError> {
    if relative.is_empty() {
        return Err(RunError::invalid_path(relative, "must not be empty"));
    }

    let mut segments = Vec::new();
    for segment in relative.split('/') {
        if segment.is_empty() {
            return Err(RunError::invalid_path(
                relative,
                "must not contain an empty segment",
            ));
        }
        if segment == "." || segment == ".." {
            return Err(RunError::invalid_path(
                relative,
                "must not contain a relative segment",
            ));
        }
        if segment.len() > MAX_SEGMENT_BYTES {
            return Err(RunError::invalid_path(relative, "has an oversized segment"));
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(RunError::invalid_path(
                relative,
                "segments accept only ASCII letters, digits, '.', '_' and '-'",
            ));
        }
        if segment.ends_with('.') {
            return Err(RunError::invalid_path(
                relative,
                "must not contain a segment ending in '.'",
            ));
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        if RESERVED_DEVICE_NAMES.contains(&stem.to_ascii_lowercase().as_str()) {
            return Err(RunError::invalid_path(
                relative,
                "must not contain a reserved device name",
            ));
        }
        segments.push(segment);
    }
    Ok(segments)
}

/// Checks a run-relative path without touching the filesystem.
pub(super) fn validate(relative: &str) -> Result<(), RunError> {
    segments(relative).map(drop)
}

/// Resolves a run-relative path under `root`, refusing every symbolic link on
/// the way so the result stays inside the run directory.
pub(super) fn resolve(root: &Path, relative: &str) -> Result<PathBuf, RunError> {
    let mut path = root.to_path_buf();
    for segment in segments(relative)? {
        path.push(segment);
        reject_link(&path)?;
    }
    Ok(path)
}

/// Resolves a run-relative path and creates the directories leading to it.
pub(super) fn reserve(root: &Path, relative: &str) -> Result<PathBuf, RunError> {
    let segments = segments(relative)?;
    let (name, parents) = segments
        .split_last()
        .ok_or_else(|| RunError::invalid_path(relative, "must not be empty"))?;
    let mut path = root.to_path_buf();
    for segment in parents {
        path.push(segment);
        create_directory(&path)?;
    }
    path.push(name);
    reject_link(&path)?;
    Ok(path)
}

/// Creates one directory, accepting only an existing real directory as the
/// already-created case.
pub(super) fn create_directory(path: &Path) -> Result<(), RunError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path).map_err(|error| RunError::io(path, error))?;
            if metadata.is_symlink() {
                Err(RunError::SymbolicLink(path.to_path_buf()))
            } else if metadata.is_dir() {
                Ok(())
            } else {
                Err(RunError::NotADirectory(path.to_path_buf()))
            }
        }
        Err(error) => Err(RunError::io(path, error)),
    }
}

fn reject_link(path: &Path) -> Result<(), RunError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => Err(RunError::SymbolicLink(path.to_path_buf())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RunError::io(path, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::segments;

    #[test]
    fn escapes_devices_and_unportable_names_are_rejected() {
        for relative in [
            "",
            "/",
            "..",
            "../outside",
            "logs/../../outside",
            "logs//adapter.log",
            "logs/",
            "./logs",
            "logs\\adapter.log",
            "logs/adapter log.txt",
            "logs/adapter.log.",
            "nul",
            "logs/NUL.txt",
            "logs/com1",
        ] {
            assert!(
                segments(relative).is_err(),
                "{relative} must not resolve inside a run directory"
            );
        }
    }

    #[test]
    fn ordinary_relative_paths_keep_their_segments() {
        assert_eq!(
            segments("attempts/0000000001-0000/outputs/proof.bin").unwrap(),
            ["attempts", "0000000001-0000", "outputs", "proof.bin"]
        );
        assert_eq!(segments("run.json").unwrap(), ["run.json"]);
        assert_eq!(segments("logs/.hidden").unwrap(), ["logs", ".hidden"]);
    }
}
