use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroU64;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};
use uriparse::{URI, URIReference};

use crate::types::number_is_unit_interval;
use crate::{ByteSize, Nanoseconds, NonEmptyString, Slug, Timestamp};

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MetadataError {
    InvalidSha256,
    InvalidConcurrency,
    EmptyCpuAffinity,
    DuplicateCpuAffinity,
    InvalidResourceProfile,
    InvalidUri(String),
    InvalidUriReference(String),
    InvalidJsonPointer(String),
    InvalidExtensionNamespace(String),
    InvalidUnitInterval,
}

impl Display for MetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSha256 => formatter.write_str("invalid lowercase SHA-256 digest"),
            Self::InvalidConcurrency => formatter.write_str("invalid run concurrency policy"),
            Self::EmptyCpuAffinity => formatter.write_str("CPU affinity must not be empty"),
            Self::DuplicateCpuAffinity => {
                formatter.write_str("CPU affinity contains duplicate indexes")
            }
            Self::InvalidResourceProfile => {
                formatter.write_str("execution mode, network access, and remote profile disagree")
            }
            Self::InvalidUri(value) => write!(formatter, "invalid absolute URI: {value}"),
            Self::InvalidUriReference(value) => write!(formatter, "invalid URI reference: {value}"),
            Self::InvalidJsonPointer(value) => write!(formatter, "invalid JSON Pointer: {value}"),
            Self::InvalidExtensionNamespace(value) => {
                write!(formatter, "invalid extension namespace: {value}")
            }
            Self::InvalidUnitInterval => formatter.write_str("value must be between zero and one"),
        }
    }
}

impl Error for MetadataError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Sha256Digest {
    algorithm: Sha256Algorithm,
    value: String,
}

impl Sha256Digest {
    /// Creates a canonical lowercase SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::InvalidSha256`] unless `value` has exactly 64
    /// lowercase hexadecimal characters.
    pub fn new(value: impl Into<String>) -> Result<Self, MetadataError> {
        let value = value.into();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self {
                algorithm: Sha256Algorithm,
                value,
            })
        } else {
            Err(MetadataError::InvalidSha256)
        }
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            algorithm: Sha256Algorithm,
            value: String,
        }

        let raw = Raw::deserialize(deserializer)?;
        let _ = raw.algorithm;
        Self::new(raw.value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Sha256Algorithm;

impl Serialize for Sha256Algorithm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("sha256")
    }
}

impl<'de> Deserialize<'de> for Sha256Algorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if String::deserialize(deserializer)? == "sha256" {
            Ok(Self)
        } else {
            Err(D::Error::custom("unsupported digest algorithm"))
        }
    }
}

macro_rules! validated_string {
    ($name:ident, $validator:expr, $error:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name(String);

        impl $name {
            /// Validates and retains a schema-constrained string.
            ///
            /// # Errors
            ///
            /// Returns the corresponding [`MetadataError`] when validation fails.
            pub fn new(value: impl Into<String>) -> Result<Self, MetadataError> {
                let value = value.into();
                if ($validator)(&value) {
                    Ok(Self(value))
                } else {
                    Err(MetadataError::$error(value))
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

validated_string!(
    AbsoluteUri,
    |value: &str| URI::try_from(value).is_ok(),
    InvalidUri
);
validated_string!(
    UriReference,
    |value: &str| URIReference::try_from(value).is_ok(),
    InvalidUriReference
);
validated_string!(JsonPointer, valid_json_pointer, InvalidJsonPointer);

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Extensions(Map<String, Value>);

impl Extensions {
    /// Validates namespaced extension keys.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::InvalidExtensionNamespace`] for the first key
    /// outside the `BenchmarkReport` v1 extension grammar.
    pub fn new(values: Map<String, Value>) -> Result<Self, MetadataError> {
        if let Some(key) = values.keys().find(|key| !valid_extension_namespace(key)) {
            Err(MetadataError::InvalidExtensionNamespace(key.clone()))
        } else {
            Ok(Self(values))
        }
    }
}

impl<'de> Deserialize<'de> for Extensions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Map::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteMetadata {
    id: Slug,
    revision: NonEmptyString,
    manifest_digest: Sha256Digest,
}

pub struct SuiteMetadataParts {
    pub id: Slug,
    pub revision: NonEmptyString,
    pub manifest_digest: Sha256Digest,
}

impl SuiteMetadata {
    #[must_use]
    pub fn new(parts: SuiteMetadataParts) -> Self {
        Self {
            id: parts.id,
            revision: parts.revision,
            manifest_digest: parts.manifest_digest,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PercentileMethod {
    Linear,
    Lower,
    Higher,
    Midpoint,
    NearestRank,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyMode {
    Isolated,
    Throughput,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Concurrency {
    mode: ConcurrencyMode,
    parallel_attempts: NonZeroU64,
}

impl Concurrency {
    /// Constructs a concurrency policy consistent with its mode.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::InvalidConcurrency`] when isolated execution
    /// has multiple attempts or throughput execution has fewer than two.
    pub fn new(
        mode: ConcurrencyMode,
        parallel_attempts: NonZeroU64,
    ) -> Result<Self, MetadataError> {
        let valid = match mode {
            ConcurrencyMode::Isolated => parallel_attempts.get() == 1,
            ConcurrencyMode::Throughput => parallel_attempts.get() >= 2,
        };
        if valid {
            Ok(Self {
                mode,
                parallel_attempts,
            })
        } else {
            Err(MetadataError::InvalidConcurrency)
        }
    }

    #[must_use]
    pub const fn mode(&self) -> ConcurrencyMode {
        self.mode
    }

    #[must_use]
    pub const fn parallel_attempts(&self) -> NonZeroU64 {
        self.parallel_attempts
    }
}

impl<'de> Deserialize<'de> for Concurrency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            mode: ConcurrencyMode,
            parallel_attempts: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        let parallel_attempts =
            NonZeroU64::new(raw.parallel_attempts).ok_or(MetadataError::InvalidConcurrency);
        Self::new(raw.mode, parallel_attempts.map_err(D::Error::custom)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPolicy {
    ordering_algorithm: NonEmptyString,
    seed: u64,
    concurrency: Concurrency,
    retry_policy: NonEmptyString,
    invalidation_policy: NonEmptyString,
    outlier_rule: NonEmptyString,
    percentile_method: PercentileMethod,
}

pub struct RunPolicyParts {
    pub ordering_algorithm: NonEmptyString,
    pub seed: u64,
    pub concurrency: Concurrency,
    pub retry_policy: NonEmptyString,
    pub invalidation_policy: NonEmptyString,
    pub outlier_rule: NonEmptyString,
    pub percentile_method: PercentileMethod,
}

impl RunPolicy {
    #[must_use]
    pub fn new(parts: RunPolicyParts) -> Self {
        Self {
            ordering_algorithm: parts.ordering_algorithm,
            seed: parts.seed,
            concurrency: parts.concurrency,
            retry_policy: parts.retry_policy,
            invalidation_policy: parts.invalidation_policy,
            outlier_rule: parts.outlier_rule,
            percentile_method: parts.percentile_method,
        }
    }

    #[must_use]
    pub const fn ordering_algorithm(&self) -> &NonEmptyString {
        &self.ordering_algorithm
    }

    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    #[must_use]
    pub const fn concurrency(&self) -> &Concurrency {
        &self.concurrency
    }

    #[must_use]
    pub const fn retry_policy(&self) -> &NonEmptyString {
        &self.retry_policy
    }

    #[must_use]
    pub const fn invalidation_policy(&self) -> &NonEmptyString {
        &self.invalidation_policy
    }

    #[must_use]
    pub const fn outlier_rule(&self) -> &NonEmptyString {
        &self.outlier_rule
    }

    #[must_use]
    pub const fn percentile_method(&self) -> PercentileMethod {
        self.percentile_method
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadMetadata {
    revision: NonEmptyString,
    specification_digest: Sha256Digest,
}

impl WorkloadMetadata {
    #[must_use]
    pub fn new(revision: NonEmptyString, specification_digest: Sha256Digest) -> Self {
        Self {
            revision,
            specification_digest,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationLane {
    Portable,
    Optimized,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputVisibility {
    Public,
    Private,
    Mixed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalInput {
    byte_length: ByteSize,
    digest: Sha256Digest,
    visibility: InputVisibility,
    preprocessing_policy: NonEmptyString,
}

pub struct CanonicalInputParts {
    pub byte_length: ByteSize,
    pub digest: Sha256Digest,
    pub visibility: InputVisibility,
    pub preprocessing_policy: NonEmptyString,
}

impl CanonicalInput {
    #[must_use]
    pub fn new(parts: CanonicalInputParts) -> Self {
        Self {
            byte_length: parts.byte_length,
            digest: parts.digest,
            visibility: parts.visibility,
            preprocessing_policy: parts.preprocessing_policy,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Commitment {
    None,
    Digest,
    Bytes,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitmentPolicy {
    input: Commitment,
    output: Commitment,
}

impl CommitmentPolicy {
    #[must_use]
    pub const fn new(input: Commitment, output: Commitment) -> Self {
        Self { input, output }
    }

    #[must_use]
    pub const fn input(&self) -> Commitment {
        self.input
    }

    #[must_use]
    pub const fn output(&self) -> Commitment {
        self.output
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleProfile {
    Cold,
    Warm,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildCacheState {
    Cold,
    Warm,
    Incremental,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupCacheState {
    Cold,
    Warm,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    profile: LifecycleProfile,
    preparation_pins: Vec<NonEmptyString>,
    build_cache_state: BuildCacheState,
    setup_cache_state: SetupCacheState,
    setup_reuse_count: u64,
}

pub struct LifecycleParts {
    pub profile: LifecycleProfile,
    pub preparation_pins: Vec<NonEmptyString>,
    pub build_cache_state: BuildCacheState,
    pub setup_cache_state: SetupCacheState,
    pub setup_reuse_count: u64,
}

impl Lifecycle {
    #[must_use]
    pub fn new(parts: LifecycleParts) -> Self {
        Self {
            profile: parts.profile,
            preparation_pins: parts.preparation_pins,
            build_cache_state: parts.build_cache_state,
            setup_cache_state: parts.setup_cache_state,
            setup_reuse_count: parts.setup_reuse_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkMetadata {
    case_id: Slug,
    workload: WorkloadMetadata,
    implementation_lane: ImplementationLane,
    canonical_input: CanonicalInput,
    expected_output_digest: Sha256Digest,
    statement: NonEmptyString,
    commitment_policy: CommitmentPolicy,
    security_target_bits: NonZeroU64,
    lifecycle: Lifecycle,
    parameters: Map<String, Value>,
}

pub struct BenchmarkMetadataParts {
    pub case_id: Slug,
    pub workload: WorkloadMetadata,
    pub implementation_lane: ImplementationLane,
    pub canonical_input: CanonicalInput,
    pub expected_output_digest: Sha256Digest,
    pub statement: NonEmptyString,
    pub commitment_policy: CommitmentPolicy,
    pub security_target_bits: NonZeroU64,
    pub lifecycle: Lifecycle,
    pub parameters: Map<String, Value>,
}

impl BenchmarkMetadata {
    #[must_use]
    pub fn new(parts: BenchmarkMetadataParts) -> Self {
        Self {
            case_id: parts.case_id,
            workload: parts.workload,
            implementation_lane: parts.implementation_lane,
            canonical_input: parts.canonical_input,
            expected_output_digest: parts.expected_output_digest,
            statement: parts.statement,
            commitment_policy: parts.commitment_policy,
            security_target_bits: parts.security_target_bits,
            lifecycle: parts.lifecycle,
            parameters: parts.parameters,
        }
    }

    #[must_use]
    pub const fn expected_output_digest(&self) -> &Sha256Digest {
        &self.expected_output_digest
    }

    #[must_use]
    pub const fn commitment_policy(&self) -> &CommitmentPolicy {
        &self.commitment_policy
    }

    #[must_use]
    pub const fn security_target_bits(&self) -> NonZeroU64 {
        self.security_target_bits
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolMetadata {
    name: NonEmptyString,
    version: NonEmptyString,
}

impl ToolMetadata {
    #[must_use]
    pub fn new(name: NonEmptyString, version: NonEmptyString) -> Self {
        Self { name, version }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerMetadata {
    name: NonEmptyString,
    version: NonEmptyString,
    flags: Vec<String>,
}

impl CompilerMetadata {
    #[must_use]
    pub fn new(name: NonEmptyString, version: NonEmptyString, flags: Vec<String>) -> Self {
        Self {
            name,
            version,
            flags,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestMetadata {
    source_revision: NonEmptyString,
    toolchain: ToolMetadata,
    compiler: CompilerMetadata,
    artifact_digest: Sha256Digest,
    optimizations: Vec<NonEmptyString>,
}

pub struct GuestMetadataParts {
    pub source_revision: NonEmptyString,
    pub toolchain: ToolMetadata,
    pub compiler: CompilerMetadata,
    pub artifact_digest: Sha256Digest,
    pub optimizations: Vec<NonEmptyString>,
}

impl GuestMetadata {
    #[must_use]
    pub fn new(parts: GuestMetadataParts) -> Self {
        Self {
            source_revision: parts.source_revision,
            toolchain: parts.toolchain,
            compiler: parts.compiler,
            artifact_digest: parts.artifact_digest,
            optimizations: parts.optimizations,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofMetadata {
    raw_format: NonEmptyString,
    final_format: NonEmptyString,
    transformation_pipeline: Vec<NonEmptyString>,
    verifier: ToolMetadata,
    parameters_version: NonEmptyString,
    parameters_digest: Sha256Digest,
}

pub struct ProofMetadataParts {
    pub raw_format: NonEmptyString,
    pub final_format: NonEmptyString,
    pub transformation_pipeline: Vec<NonEmptyString>,
    pub verifier: ToolMetadata,
    pub parameters_version: NonEmptyString,
    pub parameters_digest: Sha256Digest,
}

impl ProofMetadata {
    #[must_use]
    pub fn new(parts: ProofMetadataParts) -> Self {
        Self {
            raw_format: parts.raw_format,
            final_format: parts.final_format,
            transformation_pipeline: parts.transformation_pipeline,
            verifier: parts.verifier,
            parameters_version: parts.parameters_version,
            parameters_digest: parts.parameters_digest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineMetadata {
    id: Slug,
    name: NonEmptyString,
    version: NonEmptyString,
    adapter_revision: NonEmptyString,
    proof_system: NonEmptyString,
    backend: NonEmptyString,
    configured_security_bits: NonZeroU64,
    trust_profile: NonEmptyString,
    guest: GuestMetadata,
    proof: ProofMetadata,
    configuration: Map<String, Value>,
}

pub struct EngineMetadataParts {
    pub id: Slug,
    pub name: NonEmptyString,
    pub version: NonEmptyString,
    pub adapter_revision: NonEmptyString,
    pub proof_system: NonEmptyString,
    pub backend: NonEmptyString,
    pub configured_security_bits: NonZeroU64,
    pub trust_profile: NonEmptyString,
    pub guest: GuestMetadata,
    pub proof: ProofMetadata,
    pub configuration: Map<String, Value>,
}

impl EngineMetadata {
    #[must_use]
    pub fn new(parts: EngineMetadataParts) -> Self {
        Self {
            id: parts.id,
            name: parts.name,
            version: parts.version,
            adapter_revision: parts.adapter_revision,
            proof_system: parts.proof_system,
            backend: parts.backend,
            configured_security_bits: parts.configured_security_bits,
            trust_profile: parts.trust_profile,
            guest: parts.guest,
            proof: parts.proof,
            configuration: parts.configuration,
        }
    }

    #[must_use]
    pub const fn id(&self) -> &Slug {
        &self.id
    }

    #[must_use]
    pub const fn configured_security_bits(&self) -> NonZeroU64 {
        self.configured_security_bits
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
    Riscv64,
    Wasm32,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CpuMetadata {
    model: NonEmptyString,
    stepping: NonEmptyString,
    physical_cores: NonZeroU64,
    logical_cores: NonZeroU64,
}

impl CpuMetadata {
    #[must_use]
    pub fn new(
        model: NonEmptyString,
        stepping: NonEmptyString,
        physical_cores: NonZeroU64,
        logical_cores: NonZeroU64,
    ) -> Self {
        Self {
            model,
            stepping,
            physical_cores,
            logical_cores,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatingSystemMetadata {
    name: NonEmptyString,
    version: NonEmptyString,
    kernel: NonEmptyString,
}

impl OperatingSystemMetadata {
    #[must_use]
    pub fn new(name: NonEmptyString, version: NonEmptyString, kernel: NonEmptyString) -> Self {
        Self {
            name,
            version,
            kernel,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostMetadata {
    machine_id: NonEmptyString,
    architecture: Architecture,
    cpu: CpuMetadata,
    ram_bytes: NonZeroU64,
    accelerators: Vec<NonEmptyString>,
    storage: NonEmptyString,
    operating_system: OperatingSystemMetadata,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    firmware_or_microcode: Option<NonEmptyString>,
}

pub struct HostMetadataParts {
    pub machine_id: NonEmptyString,
    pub architecture: Architecture,
    pub cpu: CpuMetadata,
    pub ram_bytes: NonZeroU64,
    pub accelerators: Vec<NonEmptyString>,
    pub storage: NonEmptyString,
    pub operating_system: OperatingSystemMetadata,
    pub firmware_or_microcode: Option<NonEmptyString>,
}

impl HostMetadata {
    #[must_use]
    pub fn new(parts: HostMetadataParts) -> Self {
        Self {
            machine_id: parts.machine_id,
            architecture: parts.architecture,
            cpu: parts.cpu,
            ram_bytes: parts.ram_bytes,
            accelerators: parts.accelerators,
            storage: parts.storage,
            operating_system: parts.operating_system,
            firmware_or_microcode: parts.firmware_or_microcode,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceLimit {
    Limited(NonZeroU64),
    Unlimited,
}

impl Serialize for ResourceLimit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Limited(value) => value.serialize(serializer),
            Self::Unlimited => serializer.serialize_str("unlimited"),
        }
    }
}

impl<'de> Deserialize<'de> for ResourceLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Limited(NonZeroU64),
            Text(String),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Limited(value) => Ok(Self::Limited(value)),
            Raw::Text(value) if value == "unlimited" => Ok(Self::Unlimited),
            Raw::Text(_) => Err(D::Error::custom(
                "resource limit must be positive or unlimited",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnitInterval(Number);

impl UnitInterval {
    /// Creates a finite ratio between zero and one.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::InvalidUnitInterval`] for values outside the
    /// inclusive unit interval.
    pub fn new(value: Number) -> Result<Self, MetadataError> {
        if number_is_unit_interval(&value) && value.as_f64().is_some() {
            Ok(Self(value))
        } else {
            Err(MetadataError::InvalidUnitInterval)
        }
    }

    #[must_use]
    pub const fn as_number(&self) -> &Number {
        &self.0
    }
}

impl Serialize for UnitInterval {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnitInterval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Number::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProfile {
    endpoint: AbsoluteUri,
    client_region: NonEmptyString,
    endpoint_region: NonEmptyString,
    transport: NonEmptyString,
    connection_type: NonEmptyString,
    round_trip_latency_ns: Nanoseconds,
    jitter_ns: Nanoseconds,
    packet_loss_ratio: UnitInterval,
    available_bandwidth_bytes_per_second: ByteSize,
    measurement_method: NonEmptyString,
    measured_at: Timestamp,
}

pub struct RemoteProfileParts {
    pub endpoint: AbsoluteUri,
    pub client_region: NonEmptyString,
    pub endpoint_region: NonEmptyString,
    pub transport: NonEmptyString,
    pub connection_type: NonEmptyString,
    pub round_trip_latency_ns: Nanoseconds,
    pub jitter_ns: Nanoseconds,
    pub packet_loss_ratio: UnitInterval,
    pub available_bandwidth_bytes_per_second: ByteSize,
    pub measurement_method: NonEmptyString,
    pub measured_at: Timestamp,
}

impl RemoteProfile {
    #[must_use]
    pub fn new(parts: RemoteProfileParts) -> Self {
        Self {
            endpoint: parts.endpoint,
            client_region: parts.client_region,
            endpoint_region: parts.endpoint_region,
            transport: parts.transport,
            connection_type: parts.connection_type,
            round_trip_latency_ns: parts.round_trip_latency_ns,
            jitter_ns: parts.jitter_ns,
            packet_loss_ratio: parts.packet_loss_ratio,
            available_bandwidth_bytes_per_second: parts.available_bandwidth_bytes_per_second,
            measurement_method: parts.measurement_method,
            measured_at: parts.measured_at,
        }
    }

    #[must_use]
    pub const fn endpoint(&self) -> &AbsoluteUri {
        &self.endpoint
    }

    #[must_use]
    pub const fn client_region(&self) -> &NonEmptyString {
        &self.client_region
    }

    #[must_use]
    pub const fn endpoint_region(&self) -> &NonEmptyString {
        &self.endpoint_region
    }

    #[must_use]
    pub const fn transport(&self) -> &NonEmptyString {
        &self.transport
    }

    #[must_use]
    pub const fn connection_type(&self) -> &NonEmptyString {
        &self.connection_type
    }

    #[must_use]
    pub const fn round_trip_latency_ns(&self) -> Nanoseconds {
        self.round_trip_latency_ns
    }

    #[must_use]
    pub const fn jitter_ns(&self) -> Nanoseconds {
        self.jitter_ns
    }

    #[must_use]
    pub const fn packet_loss_ratio(&self) -> &UnitInterval {
        &self.packet_loss_ratio
    }

    #[must_use]
    pub const fn available_bandwidth_bytes_per_second(&self) -> ByteSize {
        self.available_bandwidth_bytes_per_second
    }

    #[must_use]
    pub const fn measurement_method(&self) -> &NonEmptyString {
        &self.measurement_method
    }

    #[must_use]
    pub const fn measured_at(&self) -> &Timestamp {
        &self.measured_at
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Resources {
    power_profile: NonEmptyString,
    cpu_affinity: Vec<u64>,
    cpu_limit: ResourceLimit,
    memory_limit_bytes: ResourceLimit,
    worker_count: NonZeroU64,
    accelerator_allocation: Vec<NonEmptyString>,
    environment_variables: BTreeMap<String, String>,
    execution_mode: ExecutionMode,
    network_access: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<RemoteProfile>,
}

pub struct ResourcesParts {
    pub power_profile: NonEmptyString,
    pub cpu_affinity: Vec<u64>,
    pub cpu_limit: ResourceLimit,
    pub memory_limit_bytes: ResourceLimit,
    pub worker_count: NonZeroU64,
    pub accelerator_allocation: Vec<NonEmptyString>,
    pub environment_variables: BTreeMap<String, String>,
    pub execution_mode: ExecutionMode,
    pub network_access: bool,
    pub remote: Option<RemoteProfile>,
}

impl Resources {
    /// Constructs resources after checking affinity and remote-mode invariants.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError`] for empty or duplicate CPU affinity, or an
    /// execution mode inconsistent with network access and remote metadata.
    pub fn new(parts: ResourcesParts) -> Result<Self, MetadataError> {
        if parts.cpu_affinity.is_empty() {
            return Err(MetadataError::EmptyCpuAffinity);
        }
        if parts
            .cpu_affinity
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != parts.cpu_affinity.len()
        {
            return Err(MetadataError::DuplicateCpuAffinity);
        }
        let profile_valid = match parts.execution_mode {
            ExecutionMode::Local => !parts.network_access && parts.remote.is_none(),
            ExecutionMode::Remote => parts.network_access && parts.remote.is_some(),
        };
        if !profile_valid {
            return Err(MetadataError::InvalidResourceProfile);
        }

        Ok(Self {
            power_profile: parts.power_profile,
            cpu_affinity: parts.cpu_affinity,
            cpu_limit: parts.cpu_limit,
            memory_limit_bytes: parts.memory_limit_bytes,
            worker_count: parts.worker_count,
            accelerator_allocation: parts.accelerator_allocation,
            environment_variables: parts.environment_variables,
            execution_mode: parts.execution_mode,
            network_access: parts.network_access,
            remote: parts.remote,
        })
    }

    #[must_use]
    pub const fn power_profile(&self) -> &NonEmptyString {
        &self.power_profile
    }

    #[must_use]
    pub fn cpu_affinity(&self) -> &[u64] {
        &self.cpu_affinity
    }

    #[must_use]
    pub const fn cpu_limit(&self) -> &ResourceLimit {
        &self.cpu_limit
    }

    #[must_use]
    pub const fn memory_limit_bytes(&self) -> &ResourceLimit {
        &self.memory_limit_bytes
    }

    #[must_use]
    pub const fn worker_count(&self) -> NonZeroU64 {
        self.worker_count
    }

    #[must_use]
    pub fn accelerator_allocation(&self) -> &[NonEmptyString] {
        &self.accelerator_allocation
    }

    #[must_use]
    pub const fn environment_variables(&self) -> &BTreeMap<String, String> {
        &self.environment_variables
    }

    #[must_use]
    pub const fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    #[must_use]
    pub const fn network_access(&self) -> bool {
        self.network_access
    }

    #[must_use]
    pub const fn remote(&self) -> Option<&RemoteProfile> {
        self.remote.as_ref()
    }
}

impl<'de> Deserialize<'de> for Resources {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            power_profile: NonEmptyString,
            cpu_affinity: Vec<u64>,
            cpu_limit: ResourceLimit,
            memory_limit_bytes: ResourceLimit,
            worker_count: NonZeroU64,
            accelerator_allocation: Vec<NonEmptyString>,
            environment_variables: BTreeMap<String, String>,
            execution_mode: ExecutionMode,
            network_access: bool,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            remote: Option<RemoteProfile>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(ResourcesParts {
            power_profile: raw.power_profile,
            cpu_affinity: raw.cpu_affinity,
            cpu_limit: raw.cpu_limit,
            memory_limit_bytes: raw.memory_limit_bytes,
            worker_count: raw.worker_count,
            accelerator_allocation: raw.accelerator_allocation,
            environment_variables: raw.environment_variables,
            execution_mode: raw.execution_mode,
            network_access: raw.network_access,
            remote: raw.remote,
        })
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClockMetadata {
    source: NonEmptyString,
    resolution_ns: NonZeroU64,
    monotonic: AlwaysTrue,
}

impl ClockMetadata {
    #[must_use]
    pub const fn new(source: NonEmptyString, resolution_ns: NonZeroU64) -> Self {
        Self {
            source,
            resolution_ns,
            monotonic: AlwaysTrue,
        }
    }
}

impl<'de> Deserialize<'de> for ClockMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source: NonEmptyString,
            resolution_ns: NonZeroU64,
            monotonic: AlwaysTrue,
        }

        let raw = Raw::deserialize(deserializer)?;
        let _ = raw.monotonic;
        Ok(Self::new(raw.source, raw.resolution_ns))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AlwaysTrue;

impl Serialize for AlwaysTrue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for AlwaysTrue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(D::Error::custom("clock must be monotonic"))
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMetadata {
    host: HostMetadata,
    resources: Resources,
    clock: ClockMetadata,
}

pub struct EnvironmentMetadataParts {
    pub host: HostMetadata,
    pub resources: Resources,
    pub clock: ClockMetadata,
}

impl EnvironmentMetadata {
    #[must_use]
    pub fn new(parts: EnvironmentMetadataParts) -> Self {
        Self {
            host: parts.host,
            resources: parts.resources,
            clock: parts.clock,
        }
    }
}

fn valid_json_pointer(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if !value.starts_with('/') {
        return false;
    }
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}

fn valid_extension_namespace(value: &str) -> bool {
    let mut segments = value.split(['.', '-']);
    let Some(first) = segments.next() else {
        return false;
    };
    if first.is_empty()
        || !first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return false;
    }
    let mut count = 0;
    for segment in segments {
        count += 1;
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return false;
        }
    }
    count > 0
}
