use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::metadata::{
    BenchmarkMetadata, EngineMetadata, EnvironmentMetadata, Extensions, JsonPointer, RunPolicy,
    Sha256Digest, SuiteMetadata, UriReference,
};
use crate::{
    ArtifactId, AttemptId, ByteSize, Measurement, NonEmptyString, Phase, Reason, ReportId, RunId,
    SchemaVersion, SemanticVersion, Slug, Timestamp,
};

mod validation;

const FAIRNESS_CONTRACT_ID: &str = "zkperf-fairness";

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReportError {
    FinishedBeforeStarted,
    EmptyPlannedOrder,
    InvalidMediaType,
    SecurityBelowTarget,
    InvalidGraph(String),
}

impl Display for ReportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FinishedBeforeStarted => {
                formatter.write_str("run finished_at must not precede started_at")
            }
            Self::EmptyPlannedOrder => formatter.write_str("run planned_order must not be empty"),
            Self::InvalidMediaType => formatter.write_str("invalid artifact media type"),
            Self::SecurityBelowTarget => {
                formatter.write_str("configured engine security is below benchmark target")
            }
            Self::InvalidGraph(message) => formatter.write_str(message),
        }
    }
}

impl Error for ReportError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibilityMetadata {
    id: FairnessContractId,
    version: SemanticVersion,
}

impl CompatibilityMetadata {
    #[must_use]
    pub const fn new(version: SemanticVersion) -> Self {
        Self {
            id: FairnessContractId,
            version,
        }
    }

    #[must_use]
    pub const fn version(&self) -> &SemanticVersion {
        &self.version
    }
}

impl<'de> Deserialize<'de> for CompatibilityMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            id: FairnessContractId,
            version: SemanticVersion,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            version: raw.version,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FairnessContractId;

impl Serialize for FairnessContractId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(FAIRNESS_CONTRACT_ID)
    }
}

impl<'de> Deserialize<'de> for FairnessContractId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == FAIRNESS_CONTRACT_ID {
            Ok(Self)
        } else {
            Err(D::Error::custom("unsupported fairness contract ID"))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkJob {
    position: u64,
    engine_id: Slug,
    phase: Phase,
    attempt_index: u64,
    warmup: bool,
}

impl BenchmarkJob {
    #[must_use]
    pub const fn new(
        position: u64,
        engine_id: Slug,
        phase: Phase,
        attempt_index: u64,
        warmup: bool,
    ) -> Self {
        Self {
            position,
            engine_id,
            phase,
            attempt_index,
            warmup,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunMetadata {
    id: RunId,
    harness_revision: NonEmptyString,
    suite: SuiteMetadata,
    started_at: Timestamp,
    finished_at: Timestamp,
    policy: RunPolicy,
    planned_order: Vec<BenchmarkJob>,
}

pub struct RunMetadataParts {
    pub id: RunId,
    pub harness_revision: NonEmptyString,
    pub suite: SuiteMetadata,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub policy: RunPolicy,
    pub planned_order: Vec<BenchmarkJob>,
}

impl RunMetadata {
    /// Constructs run metadata after checking its audit interval.
    ///
    /// # Errors
    ///
    /// Returns [`ReportError::FinishedBeforeStarted`] for a reversed interval
    /// or [`ReportError::EmptyPlannedOrder`] for an empty schedule.
    pub fn new(parts: RunMetadataParts) -> Result<Self, ReportError> {
        if !parts.started_at.precedes_or_equals(&parts.finished_at) {
            return Err(ReportError::FinishedBeforeStarted);
        }
        if parts.planned_order.is_empty() {
            return Err(ReportError::EmptyPlannedOrder);
        }
        Ok(Self {
            id: parts.id,
            harness_revision: parts.harness_revision,
            suite: parts.suite,
            started_at: parts.started_at,
            finished_at: parts.finished_at,
            policy: parts.policy,
            planned_order: parts.planned_order,
        })
    }
}

impl<'de> Deserialize<'de> for RunMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            id: RunId,
            harness_revision: NonEmptyString,
            suite: SuiteMetadata,
            started_at: Timestamp,
            finished_at: Timestamp,
            policy: RunPolicy,
            planned_order: Vec<BenchmarkJob>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(RunMetadataParts {
            id: raw.id,
            harness_revision: raw.harness_revision,
            suite: raw.suite,
            started_at: raw.started_at,
            finished_at: raw.finished_at,
            policy: raw.policy,
            planned_order: raw.planned_order,
        })
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportStatus {
    Successful,
    Failed { reason: Reason },
    TimedOut { reason: Reason },
    PartiallySupported { reason: Reason },
    Invalid { reason: Reason },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Proof,
    ProvingKey,
    VerificationKey,
    Trace,
    Log,
    Profile,
    Report,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Artifact {
    id: ArtifactId,
    name: NonEmptyString,
    kind: ArtifactKind,
    uri: UriReference,
    media_type: String,
    byte_length: ByteSize,
    digest: Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_id: Option<AttemptId>,
}

pub struct ArtifactParts {
    pub id: ArtifactId,
    pub name: NonEmptyString,
    pub kind: ArtifactKind,
    pub uri: UriReference,
    pub media_type: String,
    pub byte_length: ByteSize,
    pub digest: Sha256Digest,
    pub attempt_id: Option<AttemptId>,
}

impl Artifact {
    /// Constructs a content-addressed artifact reference.
    ///
    /// # Errors
    ///
    /// Returns [`ReportError::InvalidMediaType`] when `media_type` is not a
    /// two-part media type.
    pub fn new(parts: ArtifactParts) -> Result<Self, ReportError> {
        let valid_media_type = parts
            .media_type
            .split_once('/')
            .is_some_and(|(kind, subtype)| {
                !kind.is_empty()
                    && !subtype.is_empty()
                    && kind.bytes().all(valid_media_type_byte)
                    && subtype.bytes().all(valid_media_type_byte)
            });
        if !valid_media_type {
            return Err(ReportError::InvalidMediaType);
        }
        Ok(Self {
            id: parts.id,
            name: parts.name,
            kind: parts.kind,
            uri: parts.uri,
            media_type: parts.media_type,
            byte_length: parts.byte_length,
            digest: parts.digest,
            attempt_id: parts.attempt_id,
        })
    }
}

impl<'de> Deserialize<'de> for Artifact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            id: ArtifactId,
            name: NonEmptyString,
            kind: ArtifactKind,
            uri: UriReference,
            media_type: String,
            byte_length: ByteSize,
            digest: Sha256Digest,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            attempt_id: Option<AttemptId>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(ArtifactParts {
            id: raw.id,
            name: raw.name,
            kind: raw.kind,
            uri: raw.uri,
            media_type: raw.media_type,
            byte_length: raw.byte_length,
            digest: raw.digest,
            attempt_id: raw.attempt_id,
        })
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Warning {
    code: Slug,
    message: NonEmptyString,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    path: Option<JsonPointer>,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    attempt_id: Option<AttemptId>,
}

impl Warning {
    #[must_use]
    pub const fn new(
        code: Slug,
        message: NonEmptyString,
        path: Option<JsonPointer>,
        attempt_id: Option<AttemptId>,
    ) -> Self {
        Self {
            code,
            message,
            path,
            attempt_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchmarkReportV1 {
    schema_version: SchemaVersion,
    report_id: ReportId,
    contract: CompatibilityMetadata,
    run: RunMetadata,
    benchmark: BenchmarkMetadata,
    engine: EngineMetadata,
    environment: EnvironmentMetadata,
    measurements: Vec<Measurement>,
    artifacts: Vec<Artifact>,
    warnings: Vec<Warning>,
    status: ReportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<Extensions>,
}

pub struct BenchmarkReportV1Parts {
    pub report_id: ReportId,
    pub contract: CompatibilityMetadata,
    pub run: RunMetadata,
    pub benchmark: BenchmarkMetadata,
    pub engine: EngineMetadata,
    pub environment: EnvironmentMetadata,
    pub measurements: Vec<Measurement>,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<Warning>,
    pub status: ReportStatus,
    pub extensions: Option<Extensions>,
}

impl BenchmarkReportV1 {
    /// Constructs a validated `BenchmarkReport` v1.
    ///
    /// # Errors
    ///
    /// Returns [`ReportError`] when measurements are empty or duplicated, or
    /// when the final report status conflicts with observed outcomes.
    pub fn new(parts: BenchmarkReportV1Parts) -> Result<Self, ReportError> {
        if parts.engine.configured_security_bits() < parts.benchmark.security_target_bits() {
            return Err(ReportError::SecurityBelowTarget);
        }
        validation::validate_report(&parts)?;
        Ok(Self {
            schema_version: SchemaVersion::V1_0_0,
            report_id: parts.report_id,
            contract: parts.contract,
            run: parts.run,
            benchmark: parts.benchmark,
            engine: parts.engine,
            environment: parts.environment,
            measurements: parts.measurements,
            artifacts: parts.artifacts,
            warnings: parts.warnings,
            status: parts.status,
            extensions: parts.extensions,
        })
    }

    #[must_use]
    pub fn measurements(&self) -> &[Measurement] {
        &self.measurements
    }

    #[must_use]
    pub const fn status(&self) -> &ReportStatus {
        &self.status
    }
}

impl<'de> Deserialize<'de> for BenchmarkReportV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            schema_version: SchemaVersion,
            report_id: ReportId,
            contract: CompatibilityMetadata,
            run: RunMetadata,
            benchmark: BenchmarkMetadata,
            engine: EngineMetadata,
            environment: EnvironmentMetadata,
            measurements: Vec<Measurement>,
            artifacts: Vec<Artifact>,
            warnings: Vec<Warning>,
            status: ReportStatus,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            extensions: Option<Extensions>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let _ = raw.schema_version;
        Self::new(BenchmarkReportV1Parts {
            report_id: raw.report_id,
            contract: raw.contract,
            run: raw.run,
            benchmark: raw.benchmark,
            engine: raw.engine,
            environment: raw.environment,
            measurements: raw.measurements,
            artifacts: raw.artifacts,
            warnings: raw.warnings,
            status: raw.status,
            extensions: raw.extensions,
        })
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BenchmarkReport {
    V1(BenchmarkReportV1),
}

impl BenchmarkReport {
    /// Parses a report after selecting its exact schema version.
    ///
    /// # Errors
    ///
    /// Returns a JSON decoding error for malformed input, unsupported schema
    /// versions, or violated domain invariants.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[must_use]
    pub const fn as_v1(&self) -> &BenchmarkReportV1 {
        match self {
            Self::V1(report) => report,
        }
    }
}

impl Serialize for BenchmarkReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::V1(report) => report.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BenchmarkReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value.get("schema_version").and_then(Value::as_str) {
            Some(SchemaVersion::V1) => serde_json::from_value(value)
                .map(Self::V1)
                .map_err(D::Error::custom),
            Some(version) => Err(D::Error::custom(format!(
                "unsupported BenchmarkReport schema version {version}"
            ))),
            None => Err(D::Error::custom("missing string field schema_version")),
        }
    }
}

fn valid_media_type_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{BenchmarkReport, ReportStatus};
    use crate::{Availability, SampleStatus};

    const EXAMPLES: [&str; 4] = [
        include_str!("../../../examples/reports/successful.json"),
        include_str!("../../../examples/reports/failed.json"),
        include_str!("../../../examples/reports/timed-out.json"),
        include_str!("../../../examples/reports/partially-supported.json"),
    ];

    #[test]
    fn benchmark_report_v1_examples_round_trip_losslessly() {
        for example in EXAMPLES {
            let report = BenchmarkReport::from_json(example).unwrap();
            let expected: Value = serde_json::from_str(example).unwrap();
            let actual = serde_json::to_value(report).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn failure_timeout_and_unsupported_states_remain_explicit() {
        let failed = BenchmarkReport::from_json(EXAMPLES[1]).unwrap();
        assert!(matches!(
            failed.as_v1().status(),
            ReportStatus::Failed { .. }
        ));
        assert!(
            failed
                .as_v1()
                .measurements()
                .iter()
                .any(|measurement| measurement.has_sample_status(SampleStatus::Failed))
        );

        let timed_out = BenchmarkReport::from_json(EXAMPLES[2]).unwrap();
        assert!(
            timed_out
                .as_v1()
                .measurements()
                .iter()
                .any(|measurement| measurement.has_sample_status(SampleStatus::TimedOut))
        );

        let partially_supported = BenchmarkReport::from_json(EXAMPLES[3]).unwrap();
        assert!(
            partially_supported
                .as_v1()
                .measurements()
                .iter()
                .any(|measurement| measurement.availability() == Availability::Unavailable)
        );
    }

    #[test]
    fn rejects_unit_mixing_and_unsupported_schema_versions() {
        let mut wrong_unit: Value = serde_json::from_str(EXAMPLES[0]).unwrap();
        wrong_unit["measurements"][1]["unit"] = Value::String("count".to_owned());
        assert!(serde_json::from_value::<BenchmarkReport>(wrong_unit).is_err());

        let mut wrong_version: Value = serde_json::from_str(EXAMPLES[0]).unwrap();
        wrong_version["schema_version"] = Value::String("2.0.0".to_owned());
        assert!(serde_json::from_value::<BenchmarkReport>(wrong_version).is_err());
    }

    #[test]
    fn rejects_invalid_timing_at_the_deserialization_boundary() {
        let mut report: Value = serde_json::from_str(EXAMPLES[0]).unwrap();
        report["measurements"][0]["samples"][0]["timing"]["duration_ns"] = Value::from(1);
        assert!(serde_json::from_value::<BenchmarkReport>(report).is_err());
    }

    #[test]
    fn accepts_report_level_failure_before_the_first_attempt() {
        let mut report: Value = serde_json::from_str(EXAMPLES[3]).unwrap();
        report["measurements"]
            .as_array_mut()
            .unwrap()
            .retain(|measurement| measurement["availability"] == "unavailable");
        report["status"] = serde_json::json!({
            "outcome": "failed",
            "reason": {
                "code": "harness_start_failed",
                "message": "The harness failed before the first attempt."
            }
        });

        assert!(serde_json::from_value::<BenchmarkReport>(report).is_ok());
    }
}
