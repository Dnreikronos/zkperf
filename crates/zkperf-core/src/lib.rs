//! Engine-independent benchmark orchestration primitives.

mod digest;
mod execution;
mod init;
mod manifest;
mod measurement;
mod metadata;
mod plan;
mod report;
mod run;
mod runner;
mod sample;
mod types;

pub use execution::{ExecutionResult, execute_plan};
pub use init::initialize;
pub use manifest::{
    BenchmarkManifest, FixtureHashError, ManifestEngine, ManifestError, ManifestInput,
    ManifestOutputs, ManifestOverrides, ManifestRun, ManifestVersion, ManifestWorkload,
    OutputFormat, PhaseTimeout, ResolvedFile,
};
pub use measurement::{
    Availability, AvailableDurationMeasurement, AvailableScalarMeasurement, AvailableStatistics,
    Boundary, DurationMeasurement, DurationMeasurementPolicy, Interface, Measurement,
    MeasurementError, Metric, ProofSizeMeasurement, Rates, Ratio, ScalarMeasurement,
    ScalarQuantity, Statistics, StatisticsParts, StatusCounts, SummaryValue, TimeoutPolicy,
    UnavailableDurationMeasurement, UnavailableScalarMeasurement, UnavailableStatistics, Unit,
};
pub use metadata::{
    AbsoluteUri, Architecture, BenchmarkMetadata, BenchmarkMetadataParts, BuildCacheState,
    CanonicalInput, CanonicalInputParts, ClockMetadata, Commitment, CommitmentPolicy,
    CompilerMetadata, Concurrency, ConcurrencyMode, CpuMetadata, EngineMetadata,
    EngineMetadataParts, EnvironmentMetadata, EnvironmentMetadataParts, ExecutionMode, Extensions,
    GuestMetadata, GuestMetadataParts, HostMetadata, HostMetadataParts, ImplementationLane,
    InputVisibility, JsonPointer, Lifecycle, LifecycleParts, LifecycleProfile, MetadataError,
    OperatingSystemMetadata, PercentileMethod, ProofMetadata, ProofMetadataParts, RemoteProfile,
    RemoteProfileParts, ResourceLimit, Resources, ResourcesParts, RunPolicy, RunPolicyParts,
    SetupCacheState, Sha256Digest, SuiteMetadata, SuiteMetadataParts, ToolMetadata, UnitInterval,
    UriReference, WorkloadMetadata,
};
pub use plan::{BenchmarkPlan, FileProvenance, PlanError, PlannedJob};
pub use report::{
    Artifact, ArtifactKind, ArtifactParts, BenchmarkJob, BenchmarkReport, BenchmarkReportV1,
    BenchmarkReportV1Parts, CompatibilityMetadata, ReportError, ReportStatus, RunMetadata,
    RunMetadataParts, Warning,
};
pub use run::{
    ArtifactRequest, RunDirectory, RunError, RunOutcome, RunRecord, RunState, RunWorkspace,
};
pub use runner::{
    AdapterInvocation, CancellationToken, OperationOutcome, OperationRecord, OperationResult,
    RunnerLimits, operation_path, run_operation,
};
pub use sample::{
    Correctness, DurationObservation, DurationOutcome, ExecutionCorrectness,
    FailedProofCorrectness, ProofCorrectness, Reason, SampleError, SampleIdentity, SampleStatus,
    ScalarObservation, ScalarOutcome, VerificationVerdict,
};
pub use types::{
    ArtifactId, AttemptId, ByteSize, CombinedPhase, ComponentPhase, Count, DomainError,
    Nanoseconds, NonEmptyString, Phase, ReportId, RunId, SampleId, SchemaVersion, SemanticVersion,
    Slug, StandardPhase, Timestamp, Timing,
};

/// Adapter protocol versions that the core can negotiate.
#[must_use]
pub const fn supported_adapter_protocol_versions() -> &'static [&'static str] {
    zkperf_adapter_protocol::SUPPORTED_VERSIONS
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::supported_adapter_protocol_versions;

    #[test]
    fn supports_adapter_protocol_v1() {
        assert_eq!(supported_adapter_protocol_versions(), ["1.0.0"]);
    }
}
