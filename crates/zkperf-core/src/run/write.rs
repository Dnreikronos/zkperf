use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use cap_std::fs::{Dir, OpenOptions};

use super::{RunError, paths::RunPath};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// How many temporary names one write may try before giving up. A name is only
/// taken when another writer owns it, so a few retries are enough.
const TEMPORARY_ATTEMPTS: u32 = 16;

/// Creates a file that no earlier run or attempt can already own.
pub(super) fn create_new(path: &RunPath) -> Result<File, RunError> {
    create_file(&path.directory, &path.name, &path.path)
}

fn create_file(directory: &Dir, name: &str, path: &Path) -> Result<File, RunError> {
    directory
        .open_with(name, OpenOptions::new().write(true).create_new(true))
        .map(cap_std::fs::File::into_std)
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                RunError::AlreadyExists(path.to_path_buf())
            } else {
                RunError::io(path, error)
            }
        })
}

/// Publishes `contents` at a path nothing else owns.
///
/// The completed temporary file is linked into place, which fails when the
/// destination appeared after the caller checked for it. A rename would report
/// success while replacing that writer's evidence.
pub(super) fn publish_new(path: &RunPath, contents: &[u8]) -> Result<Publication, RunError> {
    let temporary = write_temporary(path, contents)?;
    publish_temporary(path, &temporary, sync_directory)
}

/// Publication succeeded; a subsequent sync error must not skip provenance.
#[derive(Debug)]
pub(super) struct Publication {
    durability: Result<(), RunError>,
}

impl Publication {
    fn new(path: &Path, durability: io::Result<()>) -> Self {
        Self {
            durability: durability.map_err(|source| RunError::PublishedButNotDurable {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn finish(self) -> Result<(), RunError> {
        self.durability
    }

    pub fn record<T>(self, record: impl FnOnce() -> Result<T, RunError>) -> Result<T, RunError> {
        let value = record()?;
        self.finish()?;
        Ok(value)
    }
}

/// Publishes a separate inode containing exactly the bytes that were hashed.
pub(super) fn snapshot(
    path: &RunPath,
    reader: &mut impl Read,
) -> Result<(crate::Sha256Digest, crate::ByteSize, Publication), RunError> {
    let (temporary, mut file) = create_temporary(&path.directory, &path.path)?;
    let result = crate::digest::copy_and_hash(reader, &mut file)
        .and_then(|integrity| file.sync_all().map(|()| integrity));
    drop(file);
    let integrity = match result {
        Ok(integrity) => integrity,
        Err(error) => {
            drop(path.directory.remove_file(&temporary));
            return Err(RunError::io(&path.path, error));
        }
    };
    let publication = publish_temporary(path, &temporary, sync_directory)?;
    Ok((integrity.0, integrity.1, publication))
}

fn publish_temporary(
    path: &RunPath,
    temporary: &str,
    sync: impl FnOnce(&Dir) -> io::Result<()>,
) -> Result<Publication, RunError> {
    let result = path
        .directory
        .hard_link(temporary, &path.directory, &path.name)
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                RunError::AlreadyExists(path.path.clone())
            } else {
                RunError::io(&path.path, error)
            }
        });
    // The published name now has its own link to the completed contents.
    drop(path.directory.remove_file(temporary));
    result?;
    Ok(Publication::new(&path.path, sync(&path.directory)))
}

/// Replaces `path` with `contents` so a reader observes either the previous
/// file or the whole new one.
///
/// Only the run's own canonical record is rewritten this way.
pub(super) fn replace(path: &RunPath, contents: &[u8]) -> Result<(), RunError> {
    let temporary = write_temporary(path, contents)?;
    if let Err(error) = path
        .directory
        .rename(&temporary, &path.directory, &path.name)
    {
        drop(path.directory.remove_file(&temporary));
        return Err(RunError::io(&path.path, error));
    }
    Publication::new(&path.path, sync_directory(&path.directory)).finish()
}

/// Keeps creation and cleanup relative to the same open parent directory.
fn write_temporary(path: &RunPath, contents: &[u8]) -> Result<String, RunError> {
    let (temporary, mut file) = create_temporary(&path.directory, &path.path)?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(path.directory.remove_file(&temporary));
        return Err(RunError::io(&path.path, error));
    }
    Ok(temporary)
}

fn create_temporary(directory: &Dir, path: &Path) -> Result<(String, File), RunError> {
    let mut attempt = 1;
    loop {
        let candidate = format!(
            ".zkperf-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        );
        match create_file(directory, &candidate, path) {
            Err(RunError::AlreadyExists(_)) if attempt < TEMPORARY_ATTEMPTS => attempt += 1,
            result => return result.map(|file| (candidate, file)),
        }
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Dir) -> io::Result<()> {
    directory
        // Linux directory capabilities may use O_PATH, which fsync rejects.
        // Reopen the same directory for reading without resolving its pathname.
        .open(".")
        .and_then(|handle| handle.sync_all())
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Dir) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{RunError, RunPath, create_temporary, publish_new};
    use cap_std::{ambient_authority, fs::Dir};

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
        let handle = Dir::open_ambient_dir(&directory.0, ambient_authority()).unwrap();
        let (taken, _handle) = create_temporary(&handle, &directory.0).unwrap();
        let sequence: u64 = taken
            .trim_end_matches(".tmp")
            .rsplit('-')
            .next()
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
        publish_new(
            &RunPath::at(&handle, &directory.0, "proof.bin").unwrap(),
            b"proof",
        )
        .unwrap()
        .finish()
        .unwrap();

        assert_eq!(fs::read(&planted).unwrap(), b"evidence");
        assert_eq!(fs::read(&published).unwrap(), b"proof");
    }

    #[test]
    fn publication_refuses_a_destination_another_writer_created() {
        let directory = Directory::new("publish");
        let published = directory.0.join("proof.bin");
        fs::write(&published, b"first").unwrap();

        let handle = Dir::open_ambient_dir(&directory.0, ambient_authority()).unwrap();
        let error = publish_new(
            &RunPath::at(&handle, &directory.0, "proof.bin").unwrap(),
            b"second",
        )
        .unwrap_err();

        assert!(matches!(error, RunError::AlreadyExists(_)), "{error}");
        assert_eq!(fs::read(&published).unwrap(), b"first");
        assert!(!fs::read_dir(&directory.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn a_post_publication_sync_failure_still_records_the_artifact() {
        let directory = Directory::new("sync-failure");
        let handle = Dir::open_ambient_dir(&directory.0, ambient_authority()).unwrap();
        let destination = RunPath::at(&handle, &directory.0, "proof.bin").unwrap();
        let temporary = super::write_temporary(&destination, b"proof").unwrap();
        let publication = super::publish_temporary(&destination, &temporary, |_| {
            Err(std::io::Error::other("injected directory sync failure"))
        })
        .unwrap();
        let index = directory.0.join("artifacts.jsonl");
        let error = publication
            .record(|| {
                fs::write(&index, b"{\"uri\":\"proof.bin\"}\n")
                    .map_err(|error| RunError::io(&index, error))
            })
            .unwrap_err();

        assert!(matches!(error, RunError::PublishedButNotDurable { .. }));
        assert_eq!(fs::read(&destination.path).unwrap(), b"proof");
        assert_eq!(fs::read(&index).unwrap(), b"{\"uri\":\"proof.bin\"}\n");
        assert!(!directory.0.join(temporary).exists());
    }

    #[test]
    fn a_failed_snapshot_read_leaves_no_partial_artifact() {
        use std::io::Read as _;

        struct FailedRead;

        impl std::io::Read for FailedRead {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected source read failure"))
            }
        }

        let directory = Directory::new("snapshot-read-failure");
        let handle = Dir::open_ambient_dir(&directory.0, ambient_authority()).unwrap();
        let destination = RunPath::at(&handle, &directory.0, "snapshot").unwrap();
        let mut reader = b"partial contents".as_slice().chain(FailedRead);
        assert!(super::snapshot(&destination, &mut reader).is_err());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn publication_and_cleanup_ignore_a_parent_swapped_after_resolution() {
        use crate::run::paths;

        let directory = Directory::new("parent-swap");
        let root = fs::canonicalize(&directory.0).unwrap();
        let handle = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
        let destination = paths::reserve(&handle, &root, "logs/proof.bin").unwrap();
        let outside = Directory::new("outside");

        // Interleave the swap exactly between resolution and publication.
        fs::rename(root.join("logs"), root.join("original-logs")).unwrap();
        std::os::unix::fs::symlink(&outside.0, root.join("logs")).unwrap();
        fs::write(outside.0.join("proof.bin"), b"outside evidence").unwrap();

        publish_new(&destination, b"proof")
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(
            fs::read(root.join("original-logs/proof.bin")).unwrap(),
            b"proof"
        );
        assert_eq!(
            fs::read(outside.0.join("proof.bin")).unwrap(),
            b"outside evidence"
        );
        assert_eq!(fs::read_dir(root.join("original-logs")).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 1);

        let error = publish_new(&destination, b"replacement").unwrap_err();
        assert!(matches!(error, RunError::AlreadyExists(_)));
        assert_eq!(
            fs::read(root.join("original-logs/proof.bin")).unwrap(),
            b"proof"
        );
        assert_eq!(fs::read_dir(root.join("original-logs")).unwrap().count(), 1);

        super::replace(&destination, b"canonical update").unwrap();
        assert_eq!(
            fs::read(root.join("original-logs/proof.bin")).unwrap(),
            b"canonical update"
        );
        assert_eq!(
            fs::read(outside.0.join("proof.bin")).unwrap(),
            b"outside evidence"
        );

        let stream =
            RunPath::at(&destination.directory, &root.join("logs"), "adapter.log").unwrap();
        drop(super::create_new(&stream).unwrap());
        assert!(root.join("original-logs/adapter.log").is_file());
        assert!(!outside.0.join("adapter.log").exists());
        assert!(matches!(
            paths::reserve(&handle, &root, "logs/next.bin"),
            Err(RunError::SymbolicLink(_))
        ));
    }
}
