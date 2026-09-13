use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{RUN_RECORD, RunError};
use crate::{Count, FileProvenance, RunId, Sha256Digest, Timestamp};

/// Whether a run directory holds a finished result or interrupted evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The run was created and never finished: it is still running, or it was
    /// interrupted and its partial evidence is final.
    InProgress,
    Completed,
    Failed,
}

/// How a finished run ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    Completed,
    Failed,
}

impl From<RunOutcome> for RunState {
    fn from(outcome: RunOutcome) -> Self {
        match outcome {
            RunOutcome::Completed => Self::Completed,
            RunOutcome::Failed => Self::Failed,
        }
    }
}

/// The canonical, atomically published record of one run directory.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    record_version: RecordVersion,
    run_id: RunId,
    state: RunState,
    started_at: Timestamp,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    finished_at: Option<Timestamp>,
    plan_id: String,
    manifest_path: PathBuf,
    manifest_digest: Sha256Digest,
    files: Vec<FileProvenance>,
    #[serde(default = "not_recorded")]
    environment: crate::Observed<crate::EnvironmentCapture>,
    #[serde(default = "not_recorded")]
    environment_digest: crate::Observed<Sha256Digest>,
    artifacts: Count,
}

impl RunRecord {
    pub(super) fn new(parts: RunRecordParts) -> Self {
        Self {
            record_version: RecordVersion,
            run_id: parts.run_id,
            state: RunState::InProgress,
            started_at: parts.started_at,
            finished_at: None,
            plan_id: parts.plan_id,
            manifest_path: parts.manifest_path,
            manifest_digest: parts.manifest_digest,
            files: parts.files,
            environment_digest: parts.environment.digest().into(),
            environment: parts.environment.into(),
            artifacts: Count::new(0),
        }
    }

    pub(super) fn finish(&mut self, state: RunState, finished_at: Timestamp, artifacts: Count) {
        self.state = state;
        self.finished_at = Some(finished_at);
        self.artifacts = artifacts;
    }

    /// Reads the canonical record of an existing run directory.
    ///
    /// A record that is still `in_progress` belongs either to a running
    /// harness or to an interrupted run whose evidence is now final.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is missing, unreadable, or not a valid
    /// run record.
    pub fn load(run_directory: impl AsRef<Path>) -> Result<Self, RunError> {
        let path = run_directory.as_ref().join(RUN_RECORD);
        let source = fs::read_to_string(&path).map_err(|error| RunError::io(&path, error))?;
        serde_json::from_str(&source).map_err(RunError::Serialization)
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }

    #[must_use]
    pub const fn started_at(&self) -> &Timestamp {
        &self.started_at
    }

    #[must_use]
    pub const fn finished_at(&self) -> Option<&Timestamp> {
        self.finished_at.as_ref()
    }

    #[must_use]
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    #[must_use]
    pub const fn manifest_digest(&self) -> &Sha256Digest {
        &self.manifest_digest
    }

    /// Content digests of every file the plan referenced when the run started.
    #[must_use]
    pub fn files(&self) -> &[FileProvenance] {
        &self.files
    }

    #[must_use]
    pub const fn artifacts(&self) -> Count {
        self.artifacts
    }

    #[must_use]
    pub const fn environment(&self) -> &crate::Observed<crate::EnvironmentCapture> {
        &self.environment
    }

    #[must_use]
    pub const fn environment_digest(&self) -> &crate::Observed<Sha256Digest> {
        &self.environment_digest
    }
}

fn not_recorded<T>() -> crate::Observed<T> {
    crate::Observed::gap(
        "not_recorded",
        "This run record predates environment capture.",
    )
}

pub(super) struct RunRecordParts {
    pub run_id: RunId,
    pub started_at: Timestamp,
    pub plan_id: String,
    pub manifest_path: PathBuf,
    pub manifest_digest: Sha256Digest,
    pub files: Vec<FileProvenance>,
    pub environment: crate::EnvironmentCapture,
}

/// Supported run record versions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RecordVersion;

impl RecordVersion {
    const V1: &str = "1.0.0";
}

impl Serialize for RecordVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(Self::V1)
    }
}

impl<'de> Deserialize<'de> for RecordVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        match String::deserialize(deserializer)?.as_str() {
            Self::V1 => Ok(Self),
            version => Err(D::Error::custom(format!(
                "unsupported run record version {version}"
            ))),
        }
    }
}
