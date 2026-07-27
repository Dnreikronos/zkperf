//! Validated `zkperf.toml` benchmark definitions.

mod load;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter, Write as _};
use std::fs::File;
use std::io::{self, Read};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    Commitment, ImplementationLane, InputVisibility, NonEmptyString, Resources, RunPolicy,
    Sha256Digest, Slug, StandardPhase,
};

/// A fully validated benchmark manifest.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchmarkManifest {
    manifest_path: PathBuf,
    manifest_version: ManifestVersion,
    run: ManifestRun,
    outputs: ManifestOutputs,
    workloads: Vec<ManifestWorkload>,
    engines: Vec<ManifestEngine>,
}

impl BenchmarkManifest {
    /// Loads, resolves, and validates a benchmark manifest.
    ///
    /// # Errors
    ///
    /// Returns an error with a field path when the manifest cannot be read,
    /// deserialized, resolved, or validated.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        load::load(path.as_ref())
    }

    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    #[must_use]
    pub const fn manifest_version(&self) -> ManifestVersion {
        self.manifest_version
    }

    #[must_use]
    pub const fn run(&self) -> &ManifestRun {
        &self.run
    }

    #[must_use]
    pub const fn outputs(&self) -> &ManifestOutputs {
        &self.outputs
    }

    #[must_use]
    pub fn workloads(&self) -> &[ManifestWorkload] {
        &self.workloads
    }

    #[must_use]
    pub fn engines(&self) -> &[ManifestEngine] {
        &self.engines
    }

    /// Serializes the validated definition as deterministic, pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying JSON serialization error.
    pub fn normalized_debug(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Supported `zkperf.toml` schema versions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ManifestVersion {
    V1_0_0,
}

impl ManifestVersion {
    pub const V1: &str = "1.0.0";

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1_0_0 => Self::V1,
        }
    }
}

impl Display for ManifestVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ManifestVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ManifestVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            Self::V1 => Ok(Self::V1_0_0),
            version => Err(D::Error::custom(format!(
                "unsupported benchmark manifest version {version}"
            ))),
        }
    }
}

/// Manifest-wide repetition and timeout policy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ManifestRun {
    warmups: u64,
    runs: NonZeroU64,
    policy: RunPolicy,
    resources: Resources,
    timeouts: Vec<PhaseTimeout>,
}

impl ManifestRun {
    #[must_use]
    pub const fn warmups(&self) -> u64 {
        self.warmups
    }

    #[must_use]
    pub const fn runs(&self) -> NonZeroU64 {
        self.runs
    }

    #[must_use]
    pub const fn policy(&self) -> &RunPolicy {
        &self.policy
    }

    #[must_use]
    pub const fn resources(&self) -> &Resources {
        &self.resources
    }

    #[must_use]
    pub fn timeouts(&self) -> &[PhaseTimeout] {
        &self.timeouts
    }
}

/// Timeout policy for one selected phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PhaseTimeout {
    phase: StandardPhase,
    limit_ms: NonZeroU64,
    termination_grace_ms: u64,
}

impl PhaseTimeout {
    #[must_use]
    pub const fn phase(self) -> StandardPhase {
        self.phase
    }

    #[must_use]
    pub const fn limit_ms(self) -> NonZeroU64 {
        self.limit_ms
    }

    #[must_use]
    pub const fn termination_grace_ms(self) -> u64 {
        self.termination_grace_ms
    }
}

/// Output configuration selected by the manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManifestOutputs {
    directory: PathBuf,
    formats: Vec<OutputFormat>,
}

impl ManifestOutputs {
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn formats(&self) -> &[OutputFormat] {
        &self.formats
    }
}

/// A report format selectable by benchmark manifest v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Terminal,
    Json,
    Html,
    Csv,
}

/// A manifest workload and its canonical input fixtures.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManifestWorkload {
    id: Slug,
    revision: NonEmptyString,
    specification: ResolvedFile,
    implementation_lane: ImplementationLane,
    security_target_bits: NonZeroU64,
    phases: Vec<StandardPhase>,
    inputs: Vec<ManifestInput>,
}

impl ManifestWorkload {
    #[must_use]
    pub const fn id(&self) -> &Slug {
        &self.id
    }

    #[must_use]
    pub const fn revision(&self) -> &NonEmptyString {
        &self.revision
    }

    #[must_use]
    pub const fn specification(&self) -> &ResolvedFile {
        &self.specification
    }

    #[must_use]
    pub const fn implementation_lane(&self) -> ImplementationLane {
        self.implementation_lane
    }

    #[must_use]
    pub const fn security_target_bits(&self) -> NonZeroU64 {
        self.security_target_bits
    }

    #[must_use]
    pub fn phases(&self) -> &[StandardPhase] {
        &self.phases
    }

    #[must_use]
    pub fn inputs(&self) -> &[ManifestInput] {
        &self.inputs
    }
}

/// One canonical workload input and expected output pair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManifestInput {
    id: Slug,
    fixture: ResolvedFile,
    expected_output: ResolvedFile,
    visibility: InputVisibility,
    preprocessing: NonEmptyString,
    commit_input: Commitment,
    commit_output: Commitment,
}

impl ManifestInput {
    #[must_use]
    pub const fn id(&self) -> &Slug {
        &self.id
    }

    #[must_use]
    pub const fn fixture(&self) -> &ResolvedFile {
        &self.fixture
    }

    #[must_use]
    pub const fn expected_output(&self) -> &ResolvedFile {
        &self.expected_output
    }

    #[must_use]
    pub const fn visibility(&self) -> InputVisibility {
        self.visibility
    }

    #[must_use]
    pub const fn preprocessing(&self) -> &NonEmptyString {
        &self.preprocessing
    }

    #[must_use]
    pub const fn commit_input(&self) -> Commitment {
        self.commit_input
    }

    #[must_use]
    pub const fn commit_output(&self) -> Commitment {
        self.commit_output
    }
}

/// One adapter selection and its engine-specific configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManifestEngine {
    id: Slug,
    adapter: ResolvedFile,
    proof_modes: Vec<Slug>,
    configuration: BTreeMap<String, Value>,
}

impl ManifestEngine {
    #[must_use]
    pub const fn id(&self) -> &Slug {
        &self.id
    }

    #[must_use]
    pub const fn adapter(&self) -> &ResolvedFile {
        &self.adapter
    }

    #[must_use]
    pub fn proof_modes(&self) -> &[Slug] {
        &self.proof_modes
    }

    #[must_use]
    pub const fn configuration(&self) -> &BTreeMap<String, Value> {
        &self.configuration
    }
}

/// An existing regular file resolved relative to the benchmark manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ResolvedFile(PathBuf);

impl ResolvedFile {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Hashes the current file contents with SHA-256.
    ///
    /// # Errors
    ///
    /// Returns an I/O error annotated with the resolved file path when reading
    /// fails.
    pub fn sha256(&self) -> Result<Sha256Digest, FixtureHashError> {
        let mut file =
            File::open(&self.0).map_err(|source| FixtureHashError::new(self.0.clone(), source))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 16 * 1024];

        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| FixtureHashError::new(self.0.clone(), source))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }

        let digest = hasher.finalize();
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            write!(&mut encoded, "{byte:02x}").map_err(|_| {
                FixtureHashError::new(
                    self.0.clone(),
                    io::Error::other("failed to encode SHA-256 digest"),
                )
            })?;
        }

        Sha256Digest::new(encoded).map_err(|error| {
            FixtureHashError::new(self.0.clone(), io::Error::other(error.to_string()))
        })
    }
}

/// A field-addressed manifest loading or validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestError {
    field_path: String,
    message: String,
}

impl ManifestError {
    fn new(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field_path: field_path.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub fn field_path(&self) -> &str {
        &self.field_path
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for ManifestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field_path, self.message)
    }
}

impl Error for ManifestError {}

/// An I/O error produced while content-hashing a resolved fixture.
#[derive(Debug)]
pub struct FixtureHashError {
    path: PathBuf,
    source: io::Error,
}

impl FixtureHashError {
    fn new(path: PathBuf, source: io::Error) -> Self {
        Self { path, source }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Display for FixtureHashError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not hash fixture {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl Error for FixtureHashError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}
