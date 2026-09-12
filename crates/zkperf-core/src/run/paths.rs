use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use super::{ARTIFACT_INDEX, PLAN_SNAPSHOT, RUN_RECORD, RunError};

const MAX_SEGMENT_BYTES: usize = 255;

/// Files the run directory keeps writing to. Handing one of them out as an
/// artifact would publish a digest that the next write invalidates.
const RESERVED_RUN_FILES: [&str; 3] = [RUN_RECORD, PLAN_SNAPSHOT, ARTIFACT_INDEX];

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

    if segments.len() == 1
        && RESERVED_RUN_FILES
            .iter()
            .any(|name| name.eq_ignore_ascii_case(segments[0]))
    {
        return Err(RunError::invalid_path(
            relative,
            "names a file the run directory keeps writing to",
        ));
    }
    Ok(segments)
}

/// Checks a run-relative path without touching the filesystem.
pub(super) fn validate(relative: &str) -> Result<(), RunError> {
    segments(relative).map(drop)
}

/// Confirms the run root is still the directory the run created.
///
/// The standard library has no portable way to hold a directory open and
/// resolve against that handle, so containment is checked before each
/// operation instead: the root must still be a real directory that resolves to
/// itself. A root swapped for a link, or moved under a replaced parent, is
/// refused rather than followed.
pub(super) fn verify_root(root: &Path) -> Result<(), RunError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| RunError::io(root, error))?;
    if metadata.is_symlink() {
        return Err(RunError::SymbolicLink(root.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(RunError::NotADirectory(root.to_path_buf()));
    }
    let canonical = fs::canonicalize(root).map_err(|error| RunError::io(root, error))?;
    if canonical == root {
        Ok(())
    } else {
        Err(RunError::Relocated(root.to_path_buf()))
    }
}

/// Resolves a run-relative path under `root`, refusing every symbolic link on
/// the way so the result stays inside the run directory.
pub(super) fn resolve(root: &Path, relative: &str) -> Result<PathBuf, RunError> {
    verify_root(root)?;
    let mut path = root.to_path_buf();
    for segment in segments(relative)? {
        path.push(segment);
        reject_link(&path)?;
    }
    Ok(path)
}

/// Resolves a run-relative path and creates the directories leading to it.
pub(super) fn reserve(root: &Path, relative: &str) -> Result<PathBuf, RunError> {
    verify_root(root)?;
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

/// Creates the manifest's output directory under the suite root it was
/// resolved against, refusing any link planted since the manifest was loaded.
pub(super) fn create_output_directory(
    anchor: &Path,
    directory: &Path,
) -> Result<PathBuf, RunError> {
    let suffix = directory
        .strip_prefix(anchor)
        .map_err(|_| RunError::Escapes(directory.to_path_buf()))?;
    let mut path = anchor.to_path_buf();
    for component in suffix.components() {
        let Component::Normal(segment) = component else {
            return Err(RunError::Escapes(directory.to_path_buf()));
        };
        path.push(segment);
        create_directory(&path)?;
    }
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
        assert_eq!(
            segments("reports/report.json").unwrap(),
            ["reports", "report.json"]
        );
        assert_eq!(segments("logs/.hidden").unwrap(), ["logs", ".hidden"]);
    }

    #[test]
    fn files_the_run_directory_maintains_are_not_artifact_targets() {
        for relative in ["run.json", "plan.json", "artifacts.jsonl"] {
            assert!(
                segments(relative).is_err(),
                "{relative} changes after it would be hashed"
            );
        }
        // Only the run root owns those names.
        assert!(segments("logs/run.json").is_ok());
    }
}
