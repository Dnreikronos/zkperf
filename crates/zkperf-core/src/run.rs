//! Immutable run directories, artifact provenance, and atomic result writes.

mod paths;
mod record;
mod write;

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub use record::{RunOutcome, RunRecord, RunState};

use paths::RunPath;
use record::RunRecordParts;

use crate::{
    Artifact, ArtifactId, ArtifactKind, ArtifactParts, AttemptId, BenchmarkPlan, ByteSize, Count,
    DomainError, MetadataError, NonEmptyString, PlannedJob, ReportError, RunId, Sha256Digest,
    Timestamp, UriReference,
};

/// The canonical record naming the run, its state, and its provenance.
const RUN_RECORD: &str = "run.json";
/// The redacted schedule the run was created from.
const PLAN_SNAPSHOT: &str = "plan.json";
/// One compact JSON artifact record per line, appended as evidence appears.
const ARTIFACT_INDEX: &str = "artifacts.jsonl";
/// The parent of every per-attempt adapter workspace.
const ATTEMPTS: &str = "attempts";

/// A run directory that no other run may write to or replace.
///
/// The directory is created exclusively, keeps every stored artifact
/// content-addressed, and publishes its canonical record atomically. Dropping
/// the value without [`RunDirectory::finish`] leaves the record in
/// [`RunState::InProgress`] with all partial evidence in place.
#[derive(Debug)]
pub struct RunDirectory {
    path: PathBuf,
    directory: Dir,
    record: RunRecord,
    artifacts: Vec<Artifact>,
    stored: Vec<String>,
    index: File,
}

impl RunDirectory {
    /// Creates a new run directory under the plan's resolved output directory.
    ///
    /// The directory name combines the UTC creation stamp with the derived run
    /// ID, so a new run never reuses an existing directory. Creation records
    /// the plan, the manifest digest, and the content digest of every file the
    /// plan referenced.
    ///
    /// # Errors
    ///
    /// Returns an error when the system clock is unusable, a referenced file
    /// cannot be hashed, the directory already exists, or a write fails.
    pub fn create(plan: &BenchmarkPlan) -> Result<Self, RunError> {
        static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

        let started_at = Timestamp::now()?;
        let run_id = RunId::new(derive(
            b"zkperf-run-id-v1",
            &[
                plan.id().as_bytes(),
                started_at.as_str().as_bytes(),
                &std::process::id().to_le_bytes(),
                &NEXT_RUN.fetch_add(1, Ordering::Relaxed).to_le_bytes(),
            ],
        ));

        let manifest_path = plan.manifest().manifest_path();
        let suite = manifest_path
            .parent()
            .ok_or_else(|| RunError::Escapes(manifest_path.to_path_buf()))?;
        let output = plan.manifest().outputs().directory();
        let parent = paths::create_output_directory(suite, output)?;
        let name = format!("{}-{run_id}", started_at.file_stamp());
        let path = output.join(&name);
        parent.create_dir(&name).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                RunError::AlreadyExists(path.clone())
            } else {
                RunError::io(&path, error)
            }
        })?;
        let root = paths::open_child(&parent, name.as_ref(), &path)?;
        paths::verify_root(&root, &path)?;

        let (manifest_digest, _) = crate::digest::hash_file(manifest_path)
            .map_err(|error| RunError::io(manifest_path, error))?;
        let index = write::create_new(&RunPath::at(&root, &path, ARTIFACT_INDEX)?)?;
        let directory = Self {
            record: RunRecord::new(RunRecordParts {
                run_id,
                started_at,
                plan_id: plan.id().to_owned(),
                manifest_path: manifest_path.to_path_buf(),
                manifest_digest,
                files: plan.files().to_vec(),
            }),
            path,
            directory: root,
            artifacts: Vec::new(),
            stored: Vec::new(),
            index,
        };

        let snapshot = format!("{}\n", plan.normalized_debug()?);
        write::publish_new(
            &RunPath::at(&directory.directory, &directory.path, PLAN_SNAPSHOT)?,
            snapshot.as_bytes(),
        )?;
        directory.publish_record()?;
        Ok(directory)
    }

    #[must_use]
    pub const fn id(&self) -> RunId {
        self.record.run_id()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn started_at(&self) -> &Timestamp {
        self.record.started_at()
    }

    /// Every artifact recorded so far, in the order it was stored.
    #[must_use]
    pub fn artifacts(&self) -> &[Artifact] {
        &self.artifacts
    }

    /// Writes `contents` to a run-relative path and records its integrity
    /// hash.
    ///
    /// The file is written in full before it is published under its own name,
    /// so a reader never observes a partial canonical result and never loses a
    /// file another writer published first.
    ///
    /// # Errors
    ///
    /// Returns an error when the path could escape the run directory, the path
    /// is already recorded or occupied, the artifact metadata is invalid, or a
    /// write fails.
    pub fn store(
        &mut self,
        request: &ArtifactRequest,
        contents: &[u8],
    ) -> Result<Artifact, RunError> {
        // Check the destination and the artifact before creating anything: a
        // rejected request must not leave an unrecorded file behind, and an
        // unsafe path must report why it cannot address a run directory.
        paths::validate(&request.path)?;
        let artifact = self.artifact(
            request,
            Sha256Digest::from_bytes(crate::digest::hash_bytes(contents)),
            ByteSize::new(contents.len() as u64),
        )?;
        let path = paths::reserve(&self.directory, &self.path, &request.path)?;
        write::publish_new(&path, contents)?;
        self.commit(&request.path, artifact)
    }

    /// Records an existing file inside the run directory as an artifact.
    ///
    /// This is how adapter-produced evidence, such as a proof written into an
    /// attempt's outputs root, enters the run's provenance without being
    /// copied or rewritten.
    ///
    /// # Errors
    ///
    /// Returns an error when the path could escape the run directory, is
    /// already recorded, is not a regular file, or cannot be hashed.
    pub fn adopt(&mut self, request: &ArtifactRequest) -> Result<Artifact, RunError> {
        let path = paths::resolve(&self.directory, &self.path, &request.path)?;
        let mut options = OpenOptions::new();
        // Validate the opened object without waiting for a FIFO writer first.
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let mut file = path
            .directory
            .open_with(&path.name, &options)
            .map_err(|error| RunError::io(&path.path, error))?;
        if !file
            .metadata()
            .map_err(|error| RunError::io(&path.path, error))?
            .is_file()
        {
            return Err(RunError::NotARegularFile(path.path));
        }
        let (digest, byte_length) = crate::digest::hash_reader(&mut file)
            .map_err(|error| RunError::io(&path.path, error))?;
        let artifact = self.artifact(request, digest, byte_length)?;
        self.commit(&request.path, artifact)
    }

    /// Creates a new file for evidence that is streamed while a phase runs,
    /// such as captured adapter output.
    ///
    /// The file survives an interrupted run. Call [`RunDirectory::adopt`] once
    /// it is complete to record its integrity hash.
    ///
    /// # Errors
    ///
    /// Returns an error when the path could escape the run directory, the file
    /// already exists, or it cannot be created.
    pub fn open_new(&self, relative: &str) -> Result<File, RunError> {
        write::create_new(&paths::reserve(&self.directory, &self.path, relative)?)
    }

    /// Creates the isolated workspace for one attempt at one planned job.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt already has a workspace or the
    /// workspace roots cannot be created.
    pub fn attempt(&self, job: &PlannedJob, attempt_index: u64) -> Result<RunWorkspace, RunError> {
        let relative = format!("{ATTEMPTS}/{:010}-{attempt_index:04}", job.position());
        let destination = paths::reserve(&self.directory, &self.path, &relative)?;
        let root = destination.path;
        destination
            .directory
            .create_dir(&destination.name)
            .map_err(|error| {
                if error.kind() == ErrorKind::AlreadyExists {
                    RunError::AlreadyExists(root.clone())
                } else {
                    RunError::io(&root, error)
                }
            })?;
        let directory =
            paths::open_child(&destination.directory, destination.name.as_ref(), &root)?;
        for child in [
            RunWorkspace::INPUTS,
            RunWorkspace::OUTPUTS,
            RunWorkspace::CONTROL,
        ] {
            paths::create_directory(&directory, child.as_ref(), &root.join(child))?;
        }
        Ok(RunWorkspace {
            attempt_id: AttemptId::new(derive(
                b"zkperf-attempt-id-v1",
                &[
                    self.id().get().as_bytes(),
                    job.id().as_bytes(),
                    &attempt_index.to_le_bytes(),
                ],
            )),
            inputs: root.join(RunWorkspace::INPUTS),
            outputs: root.join(RunWorkspace::OUTPUTS),
            control: root.join(RunWorkspace::CONTROL),
            root,
            relative,
        })
    }

    /// Publishes the final state of the run and returns its canonical record.
    ///
    /// # Errors
    ///
    /// Returns an error when the system clock is unusable or the record cannot
    /// be published.
    pub fn finish(mut self, outcome: RunOutcome) -> Result<RunRecord, RunError> {
        paths::verify_root(&self.directory, &self.path)?;
        let finished_at = Timestamp::now()?;
        self.index
            .sync_all()
            .map_err(|error| RunError::io(self.path.join(ARTIFACT_INDEX), error))?;
        self.record.finish(
            outcome.into(),
            finished_at,
            Count::new(self.artifacts.len() as u64),
        );
        self.publish_record()?;
        Ok(self.record.clone())
    }

    fn artifact(
        &self,
        request: &ArtifactRequest,
        digest: Sha256Digest,
        byte_length: ByteSize,
    ) -> Result<Artifact, RunError> {
        if self.stored.contains(&request.path) {
            return Err(RunError::AlreadyExists(self.path.join(&request.path)));
        }
        Artifact::new(ArtifactParts {
            id: ArtifactId::new(derive(
                b"zkperf-artifact-id-v1",
                &[self.id().get().as_bytes(), request.path.as_bytes()],
            )),
            name: request.name.clone(),
            kind: request.kind,
            uri: UriReference::new(request.path.clone())?,
            media_type: request.media_type.clone(),
            byte_length,
            digest,
            attempt_id: request.attempt_id,
        })
        .map_err(RunError::Artifact)
    }

    fn commit(&mut self, path: &str, artifact: Artifact) -> Result<Artifact, RunError> {
        let mut line = serde_json::to_vec(&artifact)?;
        line.push(b'\n');
        let index = self.path.join(ARTIFACT_INDEX);
        self.index
            .write_all(&line)
            .and_then(|()| self.index.flush())
            .map_err(|error| RunError::io(&index, error))?;

        self.stored.push(path.to_owned());
        self.artifacts.push(artifact.clone());
        Ok(artifact)
    }

    fn publish_record(&self) -> Result<(), RunError> {
        let record = format!("{}\n", serde_json::to_string_pretty(&self.record)?);
        write::replace(
            &RunPath::at(&self.directory, &self.path, RUN_RECORD)?,
            record.as_bytes(),
        )
    }
}

/// The name, kind, and destination of one artifact inside a run directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRequest {
    /// Destination relative to the run directory, using `/` separators.
    pub path: String,
    pub name: NonEmptyString,
    pub kind: ArtifactKind,
    pub media_type: String,
    pub attempt_id: Option<AttemptId>,
}

/// The three disjoint roots one adapter invocation may see.
///
/// The roots match the adapter subprocess protocol: read-only request inputs,
/// adapter-owned outputs, and host-owned control files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunWorkspace {
    attempt_id: AttemptId,
    relative: String,
    root: PathBuf,
    inputs: PathBuf,
    outputs: PathBuf,
    control: PathBuf,
}

impl RunWorkspace {
    const INPUTS: &str = "inputs";
    const OUTPUTS: &str = "outputs";
    const CONTROL: &str = "control";

    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn inputs(&self) -> &Path {
        &self.inputs
    }

    #[must_use]
    pub fn outputs(&self) -> &Path {
        &self.outputs
    }

    #[must_use]
    pub fn control(&self) -> &Path {
        &self.control
    }

    /// Run-relative path of a request input inside this workspace.
    #[must_use]
    pub fn input_path(&self, relative: &str) -> String {
        self.child_path(Self::INPUTS, relative)
    }

    /// Run-relative path of an adapter-produced output inside this workspace.
    #[must_use]
    pub fn output_path(&self, relative: &str) -> String {
        self.child_path(Self::OUTPUTS, relative)
    }

    /// Run-relative path of a host-owned control file inside this workspace.
    #[must_use]
    pub fn control_path(&self, relative: &str) -> String {
        self.child_path(Self::CONTROL, relative)
    }

    fn child_path(&self, root: &str, relative: &str) -> String {
        format!("{}/{root}/{relative}", self.relative)
    }
}

/// Derives a stable, run-scoped RFC 9562 version 8 identifier.
///
/// Identity comes from the run's own provenance rather than a random source,
/// so the same recorded inputs always name the same run, attempt, or artifact.
fn derive(domain: &[u8], ingredients: &[&[u8]]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for ingredient in ingredients {
        hasher.update((ingredient.len() as u64).to_le_bytes());
        hasher.update(ingredient);
    }
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// A run directory could not be created, extended, or published.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    InvalidPath { path: String, reason: &'static str },
    AlreadyExists(PathBuf),
    SymbolicLink(PathBuf),
    Escapes(PathBuf),
    Relocated(PathBuf),
    NotADirectory(PathBuf),
    NotARegularFile(PathBuf),
    Io { path: PathBuf, source: io::Error },
    Domain(DomainError),
    Metadata(MetadataError),
    Artifact(ReportError),
    Serialization(serde_json::Error),
}

impl RunError {
    fn invalid_path(path: &str, reason: &'static str) -> Self {
        Self::InvalidPath {
            path: path.to_owned(),
            reason,
        }
    }

    fn io(path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

impl Display for RunError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { path, reason } => {
                write!(formatter, "unsafe run artifact path {path:?}: {reason}")
            }
            Self::AlreadyExists(path) => write!(
                formatter,
                "{} already exists; runs never overwrite earlier evidence",
                path.display()
            ),
            Self::SymbolicLink(path) => write!(
                formatter,
                "{} is a symbolic link and could leave the run directory",
                path.display()
            ),
            Self::Escapes(path) => write!(
                formatter,
                "{} resolves outside the benchmark manifest directory",
                path.display()
            ),
            Self::Relocated(path) => write!(
                formatter,
                "{} no longer resolves to the directory the run created",
                path.display()
            ),
            Self::NotADirectory(path) => {
                write!(formatter, "{} is not a directory", path.display())
            }
            Self::NotARegularFile(path) => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Domain(error) => Display::fmt(error, formatter),
            Self::Metadata(error) => Display::fmt(error, formatter),
            Self::Artifact(error) => Display::fmt(error, formatter),
            Self::Serialization(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPath { .. }
            | Self::AlreadyExists(_)
            | Self::SymbolicLink(_)
            | Self::Escapes(_)
            | Self::Relocated(_)
            | Self::NotADirectory(_)
            | Self::NotARegularFile(_) => None,
            Self::Io { source, .. } => Some(source),
            Self::Domain(error) => Some(error),
            Self::Metadata(error) => Some(error),
            Self::Artifact(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
}

impl From<DomainError> for RunError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<MetadataError> for RunError {
    fn from(error: MetadataError) -> Self {
        Self::Metadata(error)
    }
}

impl From<ReportError> for RunError {
    fn from(error: ReportError) -> Self {
        Self::Artifact(error)
    }
}

impl From<serde_json::Error> for RunError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[cfg(test)]
mod tests {
    use super::derive;

    #[test]
    fn derived_identifiers_are_versioned_and_separate_their_ingredients() {
        let first = derive(b"zkperf-test-v1", &[b"ab", b"c"]);
        let second = derive(b"zkperf-test-v1", &[b"a", b"bc"]);

        assert_ne!(first, second);
        assert_eq!(first, derive(b"zkperf-test-v1", &[b"ab", b"c"]));
        assert_ne!(first, derive(b"zkperf-test-v2", &[b"ab", b"c"]));
        assert_eq!(first.get_version_num(), 8);
        assert_eq!(first.get_variant(), uuid::Variant::RFC4122);
    }
}
