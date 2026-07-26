use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::metadata::{Commitment, CommitmentPolicy, Sha256Digest};
use crate::{AttemptId, NonEmptyString, SampleId, Slug, Timestamp, Timing};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SampleError {
    FinishedBeforeStarted,
    InvalidOutcome(SampleStatus),
    MissingCorrectness,
    InapplicableCorrectness,
    InvalidCorrectnessKind,
    UnexpectedOutputDigest,
    InvalidCommitmentPayload,
}

impl Display for SampleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FinishedBeforeStarted => {
                formatter.write_str("sample finished_at must not precede started_at")
            }
            Self::InvalidOutcome(status) => {
                write!(formatter, "sample payload is invalid for status {status}")
            }
            Self::MissingCorrectness => {
                formatter.write_str("successful phase requires correctness evidence")
            }
            Self::InapplicableCorrectness => {
                formatter.write_str("phase does not allow correctness evidence")
            }
            Self::InvalidCorrectnessKind => {
                formatter.write_str("correctness payload does not match phase and outcome")
            }
            Self::UnexpectedOutputDigest => {
                formatter.write_str("correctness output digest differs from benchmark")
            }
            Self::InvalidCommitmentPayload => {
                formatter.write_str("correctness commitments differ from benchmark policy")
            }
        }
    }
}

impl Error for SampleError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    code: Slug,
    message: NonEmptyString,
}

impl Reason {
    #[must_use]
    pub const fn new(code: Slug, message: NonEmptyString) -> Self {
        Self { code, message }
    }

    #[must_use]
    pub const fn code(&self) -> &Slug {
        &self.code
    }

    #[must_use]
    pub const fn message(&self) -> &NonEmptyString {
        &self.message
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCorrectness {
    output_digest: Sha256Digest,
}

impl ExecutionCorrectness {
    #[must_use]
    pub const fn new(output_digest: Sha256Digest) -> Self {
        Self { output_digest }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationVerdict {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProofCorrectness {
    output_digest: Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_commitment_digest: Option<Sha256Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_commitment_digest: Option<Sha256Digest>,
    verification_verdict: VerificationVerdict,
}

impl<'de> Deserialize<'de> for ProofCorrectness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            output_digest: Sha256Digest,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            input_commitment_digest: Option<Sha256Digest>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            output_commitment_digest: Option<Sha256Digest>,
            verification_verdict: VerificationVerdict,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.verification_verdict != VerificationVerdict::Accepted {
            return Err(D::Error::custom(
                "successful proof verdict must be accepted",
            ));
        }
        Ok(Self {
            output_digest: raw.output_digest,
            input_commitment_digest: raw.input_commitment_digest,
            output_commitment_digest: raw.output_commitment_digest,
            verification_verdict: raw.verification_verdict,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FailedProofCorrectness {
    #[serde(skip_serializing_if = "Option::is_none")]
    output_digest: Option<Sha256Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_commitment_digest: Option<Sha256Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_commitment_digest: Option<Sha256Digest>,
    verification_verdict: VerificationVerdict,
}

impl<'de> Deserialize<'de> for FailedProofCorrectness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            output_digest: Option<Sha256Digest>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            input_commitment_digest: Option<Sha256Digest>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            output_commitment_digest: Option<Sha256Digest>,
            verification_verdict: VerificationVerdict,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.verification_verdict != VerificationVerdict::Rejected {
            return Err(D::Error::custom("failed proof verdict must be rejected"));
        }
        Ok(Self {
            output_digest: raw.output_digest,
            input_commitment_digest: raw.input_commitment_digest,
            output_commitment_digest: raw.output_commitment_digest,
            verification_verdict: raw.verification_verdict,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Correctness {
    Execution(ExecutionCorrectness),
    AcceptedProof(ProofCorrectness),
    RejectedProof(FailedProofCorrectness),
}

impl Correctness {
    #[must_use]
    pub const fn output_digest(&self) -> Option<&Sha256Digest> {
        match self {
            Self::Execution(value) => Some(&value.output_digest),
            Self::AcceptedProof(value) => Some(&value.output_digest),
            Self::RejectedProof(value) => value.output_digest.as_ref(),
        }
    }
}

impl Serialize for Correctness {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Execution(value) => value.serialize(serializer),
            Self::AcceptedProof(value) => value.serialize(serializer),
            Self::RejectedProof(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Correctness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value.get("verification_verdict").and_then(Value::as_str) {
            Some("accepted") => serde_json::from_value(value)
                .map(Self::AcceptedProof)
                .map_err(D::Error::custom),
            Some("rejected") => serde_json::from_value(value)
                .map(Self::RejectedProof)
                .map_err(D::Error::custom),
            Some(_) => Err(D::Error::custom("unknown verification verdict")),
            None => serde_json::from_value(value)
                .map(Self::Execution)
                .map_err(D::Error::custom),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleStatus {
    Success,
    Unsupported,
    Failed,
    TimedOut,
    Invalid,
}

impl Display for SampleStatus {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Success => "success",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Invalid => "invalid",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SampleIdentity {
    id: SampleId,
    attempt_id: AttemptId,
    attempt_index: u64,
    schedule_position: u64,
    warmup: bool,
    started_at: Timestamp,
    finished_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement_attempt_id: Option<AttemptId>,
}

impl SampleIdentity {
    /// Creates the metadata shared by all observations for one attempt.
    ///
    /// # Errors
    ///
    /// Returns [`SampleError::FinishedBeforeStarted`] when the audit interval
    /// is reversed.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SampleId,
        attempt_id: AttemptId,
        attempt_index: u64,
        schedule_position: u64,
        warmup: bool,
        started_at: Timestamp,
        finished_at: Timestamp,
        replacement_attempt_id: Option<AttemptId>,
    ) -> Result<Self, SampleError> {
        if !started_at.precedes_or_equals(&finished_at) {
            return Err(SampleError::FinishedBeforeStarted);
        }

        Ok(Self {
            id,
            attempt_id,
            attempt_index,
            schedule_position,
            warmup,
            started_at,
            finished_at,
            replacement_attempt_id,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SampleId {
        self.id
    }

    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt_id
    }

    #[must_use]
    pub const fn is_warmup(&self) -> bool {
        self.warmup
    }

    #[must_use]
    pub(crate) const fn attempt_index(&self) -> u64 {
        self.attempt_index
    }

    #[must_use]
    pub(crate) const fn schedule_position(&self) -> u64 {
        self.schedule_position
    }

    #[must_use]
    pub(crate) const fn started_at(&self) -> &Timestamp {
        &self.started_at
    }

    #[must_use]
    pub(crate) const fn finished_at(&self) -> &Timestamp {
        &self.finished_at
    }

    #[must_use]
    pub(crate) const fn replacement_attempt_id(&self) -> Option<AttemptId> {
        self.replacement_attempt_id
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DurationOutcome {
    Success {
        timing: Timing,
        #[serde(skip_serializing_if = "Option::is_none")]
        correctness: Option<Correctness>,
    },
    Unsupported {
        reason: Reason,
    },
    Failed {
        error_code: Slug,
        reason: Reason,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic_timing: Option<Timing>,
        #[serde(skip_serializing_if = "Option::is_none")]
        correctness: Option<Correctness>,
    },
    TimedOut {
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<Slug>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Reason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic_timing: Option<Timing>,
    },
    Invalid {
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<Slug>,
        reason: Reason,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic_timing: Option<Timing>,
    },
}

impl DurationOutcome {
    #[must_use]
    pub const fn status(&self) -> SampleStatus {
        match self {
            Self::Success { .. } => SampleStatus::Success,
            Self::Unsupported { .. } => SampleStatus::Unsupported,
            Self::Failed { .. } => SampleStatus::Failed,
            Self::TimedOut { .. } => SampleStatus::TimedOut,
            Self::Invalid { .. } => SampleStatus::Invalid,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DurationObservation {
    identity: SampleIdentity,
    outcome: DurationOutcome,
}

impl DurationObservation {
    #[must_use]
    pub const fn new(identity: SampleIdentity, outcome: DurationOutcome) -> Self {
        Self { identity, outcome }
    }

    #[must_use]
    pub const fn identity(&self) -> &SampleIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn outcome(&self) -> &DurationOutcome {
        &self.outcome
    }

    #[must_use]
    pub const fn status(&self) -> SampleStatus {
        self.outcome.status()
    }
}

impl Serialize for DurationObservation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct ObservationRef<'a> {
            #[serde(flatten)]
            identity: &'a SampleIdentity,
            #[serde(flatten)]
            outcome: &'a DurationOutcome,
        }

        ObservationRef {
            identity: &self.identity,
            outcome: &self.outcome,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DurationObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawObservation {
            id: SampleId,
            attempt_id: AttemptId,
            attempt_index: u64,
            schedule_position: u64,
            warmup: bool,
            started_at: Timestamp,
            finished_at: Timestamp,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            replacement_attempt_id: Option<AttemptId>,
            status: SampleStatus,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            timing: Option<Timing>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            correctness: Option<Correctness>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            error_code: Option<Slug>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            reason: Option<Reason>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            diagnostic_timing: Option<Timing>,
        }

        let raw = RawObservation::deserialize(deserializer)?;
        let identity = SampleIdentity::new(
            raw.id,
            raw.attempt_id,
            raw.attempt_index,
            raw.schedule_position,
            raw.warmup,
            raw.started_at,
            raw.finished_at,
            raw.replacement_attempt_id,
        )
        .map_err(D::Error::custom)?;
        let invalid = || D::Error::custom(SampleError::InvalidOutcome(raw.status));
        let outcome = match raw.status {
            SampleStatus::Success => {
                if raw.error_code.is_some()
                    || raw.reason.is_some()
                    || raw.diagnostic_timing.is_some()
                {
                    return Err(invalid());
                }
                DurationOutcome::Success {
                    timing: raw.timing.ok_or_else(invalid)?,
                    correctness: raw.correctness,
                }
            }
            SampleStatus::Unsupported => {
                if raw.timing.is_some()
                    || raw.correctness.is_some()
                    || raw.error_code.is_some()
                    || raw.diagnostic_timing.is_some()
                {
                    return Err(invalid());
                }
                DurationOutcome::Unsupported {
                    reason: raw.reason.ok_or_else(invalid)?,
                }
            }
            SampleStatus::Failed => {
                if raw.timing.is_some() {
                    return Err(invalid());
                }
                DurationOutcome::Failed {
                    error_code: raw.error_code.ok_or_else(invalid)?,
                    reason: raw.reason.ok_or_else(invalid)?,
                    diagnostic_timing: raw.diagnostic_timing,
                    correctness: raw.correctness,
                }
            }
            SampleStatus::TimedOut => {
                if raw.timing.is_some() || raw.correctness.is_some() {
                    return Err(invalid());
                }
                DurationOutcome::TimedOut {
                    error_code: raw.error_code,
                    reason: raw.reason,
                    diagnostic_timing: raw.diagnostic_timing,
                }
            }
            SampleStatus::Invalid => {
                if raw.timing.is_some() || raw.correctness.is_some() {
                    return Err(invalid());
                }
                DurationOutcome::Invalid {
                    error_code: raw.error_code,
                    reason: raw.reason.ok_or_else(invalid)?,
                    diagnostic_timing: raw.diagnostic_timing,
                }
            }
        };

        Ok(Self { identity, outcome })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ScalarOutcome<Q> {
    Success {
        value: Q,
    },
    Unsupported {
        reason: Reason,
    },
    Failed {
        error_code: Slug,
        reason: Reason,
    },
    TimedOut {
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<Slug>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Reason>,
    },
    Invalid {
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<Slug>,
        reason: Reason,
    },
}

impl<Q> ScalarOutcome<Q> {
    #[must_use]
    pub const fn status(&self) -> SampleStatus {
        match self {
            Self::Success { .. } => SampleStatus::Success,
            Self::Unsupported { .. } => SampleStatus::Unsupported,
            Self::Failed { .. } => SampleStatus::Failed,
            Self::TimedOut { .. } => SampleStatus::TimedOut,
            Self::Invalid { .. } => SampleStatus::Invalid,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScalarObservation<Q> {
    identity: SampleIdentity,
    outcome: ScalarOutcome<Q>,
}

impl<Q> ScalarObservation<Q> {
    #[must_use]
    pub const fn new(identity: SampleIdentity, outcome: ScalarOutcome<Q>) -> Self {
        Self { identity, outcome }
    }

    #[must_use]
    pub const fn identity(&self) -> &SampleIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn outcome(&self) -> &ScalarOutcome<Q> {
        &self.outcome
    }

    #[must_use]
    pub const fn status(&self) -> SampleStatus {
        self.outcome.status()
    }
}

impl<Q> Serialize for ScalarObservation<Q>
where
    Q: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct ObservationRef<'a, Q> {
            #[serde(flatten)]
            identity: &'a SampleIdentity,
            #[serde(flatten)]
            outcome: &'a ScalarOutcome<Q>,
        }

        ObservationRef {
            identity: &self.identity,
            outcome: &self.outcome,
        }
        .serialize(serializer)
    }
}

impl<'de, Q> Deserialize<'de> for ScalarObservation<Q>
where
    Q: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(bound(deserialize = "Q: Deserialize<'de>"))]
        #[serde(deny_unknown_fields)]
        struct RawObservation<Q> {
            id: SampleId,
            attempt_id: AttemptId,
            attempt_index: u64,
            schedule_position: u64,
            warmup: bool,
            started_at: Timestamp,
            finished_at: Timestamp,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            replacement_attempt_id: Option<AttemptId>,
            status: SampleStatus,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            value: Option<Q>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            error_code: Option<Slug>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            reason: Option<Reason>,
        }

        let raw = RawObservation::deserialize(deserializer)?;
        let identity = SampleIdentity::new(
            raw.id,
            raw.attempt_id,
            raw.attempt_index,
            raw.schedule_position,
            raw.warmup,
            raw.started_at,
            raw.finished_at,
            raw.replacement_attempt_id,
        )
        .map_err(D::Error::custom)?;
        let invalid = || D::Error::custom(SampleError::InvalidOutcome(raw.status));
        let outcome = match raw.status {
            SampleStatus::Success => {
                if raw.error_code.is_some() || raw.reason.is_some() {
                    return Err(invalid());
                }
                ScalarOutcome::Success {
                    value: raw.value.ok_or_else(invalid)?,
                }
            }
            SampleStatus::Unsupported => {
                if raw.value.is_some() || raw.error_code.is_some() {
                    return Err(invalid());
                }
                ScalarOutcome::Unsupported {
                    reason: raw.reason.ok_or_else(invalid)?,
                }
            }
            SampleStatus::Failed => {
                if raw.value.is_some() {
                    return Err(invalid());
                }
                ScalarOutcome::Failed {
                    error_code: raw.error_code.ok_or_else(invalid)?,
                    reason: raw.reason.ok_or_else(invalid)?,
                }
            }
            SampleStatus::TimedOut => {
                if raw.value.is_some() {
                    return Err(invalid());
                }
                ScalarOutcome::TimedOut {
                    error_code: raw.error_code,
                    reason: raw.reason,
                }
            }
            SampleStatus::Invalid => {
                if raw.value.is_some() {
                    return Err(invalid());
                }
                ScalarOutcome::Invalid {
                    error_code: raw.error_code,
                    reason: raw.reason.ok_or_else(invalid)?,
                }
            }
        };

        Ok(Self { identity, outcome })
    }
}

impl DurationObservation {
    /// Validates correctness evidence against phase and benchmark policy.
    ///
    /// # Errors
    ///
    /// Returns [`SampleError`] when correctness is missing, inapplicable, has
    /// the wrong outcome kind, carries an unexpected output, or violates the
    /// declared commitment policy.
    pub fn validate_correctness(
        &self,
        phase: &crate::Phase,
        expected_output: &Sha256Digest,
        commitment_policy: &CommitmentPolicy,
    ) -> Result<(), SampleError> {
        match &self.outcome {
            DurationOutcome::Success { correctness, .. } if phase.is_proof_related() => {
                let Some(Correctness::AcceptedProof(correctness)) = correctness else {
                    return Err(if correctness.is_none() {
                        SampleError::MissingCorrectness
                    } else {
                        SampleError::InvalidCorrectnessKind
                    });
                };
                if &correctness.output_digest != expected_output {
                    return Err(SampleError::UnexpectedOutputDigest);
                }
                validate_commitment(
                    commitment_policy.input(),
                    correctness.input_commitment_digest.as_ref(),
                )?;
                validate_commitment(
                    commitment_policy.output(),
                    correctness.output_commitment_digest.as_ref(),
                )
            }
            DurationOutcome::Success { correctness, .. } if phase.is_execution_only() => {
                let Some(Correctness::Execution(correctness)) = correctness else {
                    return Err(if correctness.is_none() {
                        SampleError::MissingCorrectness
                    } else {
                        SampleError::InvalidCorrectnessKind
                    });
                };
                if &correctness.output_digest == expected_output {
                    Ok(())
                } else {
                    Err(SampleError::UnexpectedOutputDigest)
                }
            }
            DurationOutcome::Failed { correctness, .. } if phase.is_proof_related() => {
                if correctness
                    .as_ref()
                    .is_none_or(|value| matches!(value, Correctness::RejectedProof(_)))
                {
                    Ok(())
                } else {
                    Err(SampleError::InvalidCorrectnessKind)
                }
            }
            DurationOutcome::Failed { correctness, .. } if phase.is_execution_only() => {
                if correctness
                    .as_ref()
                    .is_none_or(|value| matches!(value, Correctness::Execution(_)))
                {
                    Ok(())
                } else {
                    Err(SampleError::InvalidCorrectnessKind)
                }
            }
            DurationOutcome::Success { correctness, .. }
            | DurationOutcome::Failed { correctness, .. } => {
                if correctness.is_none() {
                    Ok(())
                } else {
                    Err(SampleError::InapplicableCorrectness)
                }
            }
            _ => Ok(()),
        }
    }
}

fn validate_commitment(
    policy: Commitment,
    digest: Option<&Sha256Digest>,
) -> Result<(), SampleError> {
    if (policy == Commitment::None) == digest.is_none() {
        Ok(())
    } else {
        Err(SampleError::InvalidCommitmentPayload)
    }
}
