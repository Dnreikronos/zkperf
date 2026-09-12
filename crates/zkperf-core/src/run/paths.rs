use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::{ambient_authority, fs::Dir};

use super::{ARTIFACT_INDEX, PLAN_SNAPSHOT, RUN_RECORD, RunError, SNAPSHOTS};

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

    if segments[0].eq_ignore_ascii_case(SNAPSHOTS) {
        return Err(RunError::invalid_path(
            relative,
            "names the run's artifact snapshot storage",
        ));
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

/// A single name inside an open directory. `path` is only for diagnostics;
/// filesystem operations must use `directory` and `name` together.
pub(super) struct RunPath {
    pub directory: Dir,
    pub name: String,
    pub path: PathBuf,
}

impl RunPath {
    pub fn at(directory: &Dir, root: &Path, name: &str) -> Result<Self, RunError> {
        Ok(Self {
            directory: directory
                .try_clone()
                .map_err(|error| RunError::io(root, error))?,
            name: name.to_owned(),
            path: root.join(name),
        })
    }
}

/// This check diagnoses relocation. Subsequent I/O still uses the original
/// handle, so a rename after this check cannot redirect that I/O.
pub(super) fn verify_root(directory: &Dir, root: &Path) -> Result<(), RunError> {
    let current = open_absolute(root)?;
    let current = current
        .dir_metadata()
        .map_err(|error| RunError::io(root, error))?;
    let original = directory
        .dir_metadata()
        .map_err(|error| RunError::io(root, error))?;
    if current.dev() != original.dev() || current.ino() != original.ino() {
        return Err(RunError::Relocated(root.to_path_buf()));
    }
    Ok(())
}

pub(super) fn resolve(directory: &Dir, root: &Path, relative: &str) -> Result<RunPath, RunError> {
    parent(directory, root, relative, false)
}

pub(super) fn reserve(directory: &Dir, root: &Path, relative: &str) -> Result<RunPath, RunError> {
    parent(directory, root, relative, true)
}

fn parent(directory: &Dir, root: &Path, relative: &str, create: bool) -> Result<RunPath, RunError> {
    let segments = segments(relative)?;
    let (name, parents) = segments
        .split_last()
        .ok_or_else(|| RunError::invalid_path(relative, "must not be empty"))?;
    verify_root(directory, root)?;
    let mut directory = directory
        .try_clone()
        .map_err(|error| RunError::io(root, error))?;
    let mut path = root.to_path_buf();
    for segment in parents {
        path.push(segment);
        directory = if create {
            create_directory(&directory, segment.as_ref(), &path)?
        } else {
            open_child(&directory, segment.as_ref(), &path)?
        };
    }
    path.push(name);
    reject_link(&directory, name.as_ref(), &path)?;
    Ok(RunPath {
        directory,
        name: (*name).to_owned(),
        path,
    })
}

/// Opens a canonical absolute path without following any replaced ancestor.
fn open_absolute(root: &Path) -> Result<Dir, RunError> {
    let anchor = root
        .ancestors()
        .last()
        .ok_or_else(|| RunError::Escapes(root.to_path_buf()))?;
    if !anchor.is_absolute() {
        return Err(RunError::Escapes(root.to_path_buf()));
    }
    let mut directory = Dir::open_ambient_dir(anchor, ambient_authority())
        .map_err(|error| RunError::io(anchor, error))?;
    let mut path = anchor.to_path_buf();
    for component in root
        .strip_prefix(anchor)
        .map_err(|_| RunError::Escapes(root.to_path_buf()))?
        .components()
    {
        let Component::Normal(name) = component else {
            return Err(RunError::Escapes(root.to_path_buf()));
        };
        path.push(name);
        directory = open_child(&directory, name, &path)?;
    }
    Ok(directory)
}

pub(super) fn create_output_directory(anchor: &Path, output: &Path) -> Result<Dir, RunError> {
    let suffix = output
        .strip_prefix(anchor)
        .map_err(|_| RunError::Escapes(output.to_path_buf()))?;
    let mut directory = open_absolute(anchor)?;
    let mut path = anchor.to_path_buf();
    for component in suffix.components() {
        let Component::Normal(segment) = component else {
            return Err(RunError::Escapes(output.to_path_buf()));
        };
        path.push(segment);
        directory = create_directory(&directory, segment, &path)?;
    }
    Ok(directory)
}

pub(super) fn create_directory(
    directory: &Dir,
    name: &OsStr,
    path: &Path,
) -> Result<Dir, RunError> {
    match directory.create_dir(name) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(RunError::io(path, error)),
    }
    open_child(directory, name, path)
}

pub(super) fn open_child(directory: &Dir, name: &OsStr, path: &Path) -> Result<Dir, RunError> {
    reject_link(directory, name, path)?;
    directory
        .open_dir_nofollow(name)
        .map_err(|error| RunError::io(path, error))
}

fn reject_link(directory: &Dir, name: &OsStr, path: &Path) -> Result<(), RunError> {
    match directory.symlink_metadata(name) {
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
