use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroU64;

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Number, Value};

use crate::metadata::BenchmarkMetadata;
use crate::{
    AttemptId, ByteSize, Count, DurationObservation, Nanoseconds, NonEmptyString, Phase, Reason,
    SampleStatus, ScalarObservation,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Duration,
    PeakResidentMemory,
    ProofSize,
    ProvingKeySize,
    VerificationKeySize,
    ConstraintCount,
    CycleCount,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Nanosecond,
    Byte,
    Count,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MeasurementError {
    EmptySamples,
    InvalidUnit { expected: Unit, actual: Unit },
    MissingProofSizePhase,
    ZeroTimeout,
    InvalidStatistics(&'static str),
    InvalidCorrectness,
}

impl Display for MeasurementError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySamples => formatter.write_str("available measurement requires samples"),
            Self::InvalidUnit { expected, actual } => {
                write!(
                    formatter,
                    "metric requires unit {expected:?}, got {actual:?}"
                )
            }
            Self::MissingProofSizePhase => {
                formatter.write_str("proof_size measurement requires a phase")
            }
            Self::ZeroTimeout => formatter.write_str("timeout limit must be greater than zero"),
            Self::InvalidStatistics(message) => {
                write!(formatter, "invalid statistics: {message}")
            }
            Self::InvalidCorrectness => {
                formatter.write_str("duration correctness contradicts benchmark metadata")
            }
        }
    }
}

impl Error for MeasurementError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    InProcess,
    Subprocess,
    Remote,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    interface: Interface,
    start: NonEmptyString,
    stop: NonEmptyString,
}

impl Boundary {
    #[must_use]
    pub const fn new(interface: Interface, start: NonEmptyString, stop: NonEmptyString) -> Self {
        Self {
            interface,
            start,
            stop,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TimeoutPolicy {
    limit_ns: NonZeroU64,
    enforcement: NonEmptyString,
    termination_grace_ns: Nanoseconds,
}

impl TimeoutPolicy {
    /// Creates a timeout policy with a strictly positive limit.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::ZeroTimeout`] when `limit_ns` is zero.
    pub fn new(
        limit_ns: Nanoseconds,
        enforcement: NonEmptyString,
        termination_grace_ns: Nanoseconds,
    ) -> Result<Self, MeasurementError> {
        let limit_ns = NonZeroU64::new(limit_ns.get()).ok_or(MeasurementError::ZeroTimeout)?;
        Ok(Self {
            limit_ns,
            enforcement,
            termination_grace_ns,
        })
    }
}

impl<'de> Deserialize<'de> for TimeoutPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            limit_ns: Nanoseconds,
            enforcement: NonEmptyString,
            termination_grace_ns: Nanoseconds,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.limit_ns, raw.enforcement, raw.termination_grace_ns).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurationMeasurementPolicy {
    warmup_count: u64,
    measured_trial_count: NonZeroU64,
    timeout: TimeoutPolicy,
}

impl DurationMeasurementPolicy {
    #[must_use]
    pub const fn new(
        warmup_count: u64,
        measured_trial_count: NonZeroU64,
        timeout: TimeoutPolicy,
    ) -> Self {
        Self {
            warmup_count,
            measured_trial_count,
            timeout,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusCounts {
    pub success: u64,
    pub unsupported: u64,
    pub failed: u64,
    pub timed_out: u64,
    pub invalid: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ratio(Number);

impl Ratio {
    /// Creates a ratio in the inclusive range zero through one.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::InvalidStatistics`] for a value outside
    /// the unit interval.
    pub fn new(value: Number) -> Result<Self, MeasurementError> {
        let numeric = value
            .as_f64()
            .ok_or(MeasurementError::InvalidStatistics("ratio is not finite"))?;
        if (0.0..=1.0).contains(&numeric) {
            Ok(Self(value))
        } else {
            Err(MeasurementError::InvalidStatistics(
                "ratio must be between zero and one",
            ))
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> f64 {
        self.0.as_f64().unwrap_or(f64::INFINITY)
    }
}

impl Serialize for Ratio {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Ratio {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Number::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rates {
    failure: Ratio,
    timeout: Ratio,
}

impl Rates {
    #[must_use]
    pub const fn new(failure: Ratio, timeout: Ratio) -> Self {
        Self { failure, timeout }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryValue<Q> {
    value: Number,
    quantity: PhantomData<Q>,
}

impl<Q> SummaryValue<Q> {
    /// Creates a non-negative statistical value carrying unit type `Q`.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::InvalidStatistics`] when `value` is
    /// negative.
    pub fn new(value: Number) -> Result<Self, MeasurementError> {
        let numeric = value.as_f64().ok_or(MeasurementError::InvalidStatistics(
            "summary value is not finite",
        ))?;
        if numeric < 0.0 {
            Err(MeasurementError::InvalidStatistics(
                "summary value must be non-negative",
            ))
        } else {
            Ok(Self {
                value,
                quantity: PhantomData,
            })
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> f64 {
        self.value.as_f64().unwrap_or(f64::INFINITY)
    }
}

impl<Q> Serialize for SummaryValue<Q> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.serialize(serializer)
    }
}

impl<'de, Q> Deserialize<'de> for SummaryValue<Q> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Number::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AvailableStatistics<Q> {
    sample_count: u64,
    minimum: SummaryValue<Q>,
    maximum: SummaryValue<Q>,
    median: SummaryValue<Q>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mean: Option<SummaryValue<Q>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    standard_deviation: Option<SummaryValue<Q>>,
    percentiles: BTreeMap<String, SummaryValue<Q>>,
    included_attempt_ids: Vec<AttemptId>,
    excluded_warmup_attempt_ids: Vec<AttemptId>,
    flagged_outlier_attempt_ids: Vec<AttemptId>,
    status_counts: StatusCounts,
    rates: Rates,
}

pub struct StatisticsParts<Q> {
    pub sample_count: u64,
    pub minimum: SummaryValue<Q>,
    pub maximum: SummaryValue<Q>,
    pub median: SummaryValue<Q>,
    pub mean: Option<SummaryValue<Q>>,
    pub standard_deviation: Option<SummaryValue<Q>>,
    pub percentiles: BTreeMap<String, SummaryValue<Q>>,
    pub included_attempt_ids: Vec<AttemptId>,
    pub excluded_warmup_attempt_ids: Vec<AttemptId>,
    pub flagged_outlier_attempt_ids: Vec<AttemptId>,
    pub status_counts: StatusCounts,
    pub rates: Rates,
}

impl<Q> AvailableStatistics<Q> {
    /// Validates and constructs an available statistical summary.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::InvalidStatistics`] for an empty
    /// population, inconsistent bounds, invalid percentile keys, duplicate
    /// attempt IDs, or a sample count that does not match included attempts.
    pub fn new(parts: StatisticsParts<Q>) -> Result<Self, MeasurementError> {
        if parts.sample_count == 0 {
            return Err(MeasurementError::InvalidStatistics(
                "available summary requires at least one sample",
            ));
        }
        let sample_count = usize::try_from(parts.sample_count).map_err(|_| {
            MeasurementError::InvalidStatistics("sample_count does not fit this platform")
        })?;
        if parts.included_attempt_ids.len() != sample_count {
            return Err(MeasurementError::InvalidStatistics(
                "sample_count must match included_attempt_ids",
            ));
        }
        if parts.minimum.as_f64() > parts.median.as_f64()
            || parts.median.as_f64() > parts.maximum.as_f64()
        {
            return Err(MeasurementError::InvalidStatistics(
                "minimum, median, and maximum are not ordered",
            ));
        }
        if parts.percentiles.is_empty()
            || !parts.percentiles.keys().all(|key| valid_percentile(key))
        {
            return Err(MeasurementError::InvalidStatistics(
                "percentiles must use BenchmarkReport percentile keys",
            ));
        }
        for ids in [
            &parts.included_attempt_ids,
            &parts.excluded_warmup_attempt_ids,
            &parts.flagged_outlier_attempt_ids,
        ] {
            if ids.iter().copied().collect::<HashSet<_>>().len() != ids.len() {
                return Err(MeasurementError::InvalidStatistics(
                    "attempt ID lists must not contain duplicates",
                ));
            }
        }

        Ok(Self {
            sample_count: parts.sample_count,
            minimum: parts.minimum,
            maximum: parts.maximum,
            median: parts.median,
            mean: parts.mean,
            standard_deviation: parts.standard_deviation,
            percentiles: parts.percentiles,
            included_attempt_ids: parts.included_attempt_ids,
            excluded_warmup_attempt_ids: parts.excluded_warmup_attempt_ids,
            flagged_outlier_attempt_ids: parts.flagged_outlier_attempt_ids,
            status_counts: parts.status_counts,
            rates: parts.rates,
        })
    }
}

impl<'de, Q> Deserialize<'de> for AvailableStatistics<Q> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(bound(deserialize = ""), deny_unknown_fields)]
        struct Raw<Q> {
            sample_count: u64,
            minimum: SummaryValue<Q>,
            maximum: SummaryValue<Q>,
            median: SummaryValue<Q>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            mean: Option<SummaryValue<Q>>,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            standard_deviation: Option<SummaryValue<Q>>,
            percentiles: BTreeMap<String, SummaryValue<Q>>,
            included_attempt_ids: Vec<AttemptId>,
            excluded_warmup_attempt_ids: Vec<AttemptId>,
            flagged_outlier_attempt_ids: Vec<AttemptId>,
            status_counts: StatusCounts,
            rates: Rates,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(StatisticsParts {
            sample_count: raw.sample_count,
            minimum: raw.minimum,
            maximum: raw.maximum,
            median: raw.median,
            mean: raw.mean,
            standard_deviation: raw.standard_deviation,
            percentiles: raw.percentiles,
            included_attempt_ids: raw.included_attempt_ids,
            excluded_warmup_attempt_ids: raw.excluded_warmup_attempt_ids,
            flagged_outlier_attempt_ids: raw.flagged_outlier_attempt_ids,
            status_counts: raw.status_counts,
            rates: raw.rates,
        })
        .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnavailableStatistics {
    reason: Reason,
    status_counts: StatusCounts,
    rates: Rates,
}

impl UnavailableStatistics {
    #[must_use]
    pub const fn new(reason: Reason, status_counts: StatusCounts, rates: Rates) -> Self {
        Self {
            reason,
            status_counts,
            rates,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statistics<Q> {
    Available(AvailableStatistics<Q>),
    Unavailable(UnavailableStatistics),
}

impl<Q> Serialize for Statistics<Q>
where
    Q: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct AvailableRef<'a, Q> {
            status: Availability,
            #[serde(flatten)]
            fields: &'a AvailableStatistics<Q>,
        }

        #[derive(Serialize)]
        struct UnavailableRef<'a> {
            status: Availability,
            #[serde(flatten)]
            fields: &'a UnavailableStatistics,
        }

        match self {
            Self::Available(fields) => AvailableRef {
                status: Availability::Available,
                fields,
            }
            .serialize(serializer),
            Self::Unavailable(fields) => UnavailableRef {
                status: Availability::Unavailable,
                fields,
            }
            .serialize(serializer),
        }
    }
}

impl<'de, Q> Deserialize<'de> for Statistics<Q> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let status = take_tag::<D::Error>(&mut value, "status")?;
        match status.as_str() {
            "available" => serde_json::from_value(value)
                .map(Self::Available)
                .map_err(D::Error::custom),
            "unavailable" => serde_json::from_value(value)
                .map(Self::Unavailable)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom("unknown statistics status")),
        }
    }
}

pub trait ScalarQuantity:
    Clone + std::fmt::Debug + PartialEq + Serialize + DeserializeOwned
{
    const UNIT: Unit;

    fn canonical_value(&self) -> u64;
}

impl ScalarQuantity for ByteSize {
    const UNIT: Unit = Unit::Byte;

    fn canonical_value(&self) -> u64 {
        self.get()
    }
}

impl ScalarQuantity for Count {
    const UNIT: Unit = Unit::Count;

    fn canonical_value(&self) -> u64 {
        self.get()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AvailableDurationMeasurement {
    phase: Phase,
    boundary: Boundary,
    policy: DurationMeasurementPolicy,
    samples: Vec<DurationObservation>,
    statistics: Statistics<Nanoseconds>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnavailableDurationMeasurement {
    phase: Phase,
    reason: Reason,
    policy: DurationMeasurementPolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DurationMeasurement {
    Available(Box<AvailableDurationMeasurement>),
    Unavailable(UnavailableDurationMeasurement),
}

impl DurationMeasurement {
    /// Constructs an available duration measurement.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::EmptySamples`] when no observations exist.
    pub fn available(
        phase: Phase,
        boundary: Boundary,
        policy: DurationMeasurementPolicy,
        samples: Vec<DurationObservation>,
        statistics: Statistics<Nanoseconds>,
    ) -> Result<Self, MeasurementError> {
        if samples.is_empty() {
            return Err(MeasurementError::EmptySamples);
        }
        Ok(Self::Available(Box::new(AvailableDurationMeasurement {
            phase,
            boundary,
            policy,
            samples,
            statistics,
        })))
    }

    #[must_use]
    pub const fn unavailable(
        phase: Phase,
        reason: Reason,
        policy: DurationMeasurementPolicy,
    ) -> Self {
        Self::Unavailable(UnavailableDurationMeasurement {
            phase,
            reason,
            policy,
        })
    }

    #[must_use]
    pub const fn phase(&self) -> &Phase {
        match self {
            Self::Available(value) => &value.phase,
            Self::Unavailable(value) => &value.phase,
        }
    }

    #[must_use]
    pub const fn availability(&self) -> Availability {
        match self {
            Self::Available(_) => Availability::Available,
            Self::Unavailable(_) => Availability::Unavailable,
        }
    }

    fn has_status(&self, status: SampleStatus) -> bool {
        match self {
            Self::Available(value) => value.samples.iter().any(|sample| sample.status() == status),
            Self::Unavailable(_) => false,
        }
    }

    const fn has_samples(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    fn validate_correctness(&self, benchmark: &BenchmarkMetadata) -> Result<(), MeasurementError> {
        if let Self::Available(value) = self {
            for sample in &value.samples {
                sample
                    .validate_correctness(
                        &value.phase,
                        benchmark.expected_output_digest(),
                        benchmark.commitment_policy(),
                    )
                    .map_err(|_| MeasurementError::InvalidCorrectness)?;
            }
        }
        Ok(())
    }
}

impl Serialize for DurationMeasurement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct MeasurementRef<'a, T> {
            unit: Unit,
            availability: Availability,
            #[serde(flatten)]
            fields: &'a T,
        }

        match self {
            Self::Available(fields) => MeasurementRef {
                unit: Unit::Nanosecond,
                availability: Availability::Available,
                fields,
            }
            .serialize(serializer),
            Self::Unavailable(fields) => MeasurementRef {
                unit: Unit::Nanosecond,
                availability: Availability::Unavailable,
                fields,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for DurationMeasurement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct AvailableRaw {
            unit: Unit,
            availability: Availability,
            phase: Phase,
            boundary: Boundary,
            policy: DurationMeasurementPolicy,
            samples: Vec<DurationObservation>,
            statistics: Statistics<Nanoseconds>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnavailableRaw {
            unit: Unit,
            availability: Availability,
            phase: Phase,
            reason: Reason,
            policy: DurationMeasurementPolicy,
        }

        let value = Value::deserialize(deserializer)?;
        match read_tag::<D::Error>(&value, "availability")?.as_str() {
            "available" => {
                let raw: AvailableRaw = serde_json::from_value(value).map_err(D::Error::custom)?;
                expect_unit::<D::Error>(raw.unit, Unit::Nanosecond)?;
                if raw.availability != Availability::Available {
                    return Err(D::Error::custom("invalid availability"));
                }
                Self::available(
                    raw.phase,
                    raw.boundary,
                    raw.policy,
                    raw.samples,
                    raw.statistics,
                )
                .map_err(D::Error::custom)
            }
            "unavailable" => {
                let raw: UnavailableRaw =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                expect_unit::<D::Error>(raw.unit, Unit::Nanosecond)?;
                if raw.availability != Availability::Unavailable {
                    return Err(D::Error::custom("invalid availability"));
                }
                Ok(Self::unavailable(raw.phase, raw.reason, raw.policy))
            }
            _ => Err(D::Error::custom("unknown measurement availability")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AvailableScalarMeasurement<Q> {
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<Phase>,
    samples: Vec<ScalarObservation<Q>>,
    statistics: Statistics<Q>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnavailableScalarMeasurement {
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<Phase>,
    reason: Reason,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScalarMeasurement<Q> {
    Available(Box<AvailableScalarMeasurement<Q>>),
    Unavailable(UnavailableScalarMeasurement),
}

impl<Q> ScalarMeasurement<Q> {
    /// Constructs an available scalar measurement.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::EmptySamples`] when no observations exist.
    pub fn available(
        phase: Option<Phase>,
        samples: Vec<ScalarObservation<Q>>,
        statistics: Statistics<Q>,
    ) -> Result<Self, MeasurementError> {
        if samples.is_empty() {
            return Err(MeasurementError::EmptySamples);
        }
        Ok(Self::Available(Box::new(AvailableScalarMeasurement {
            phase,
            samples,
            statistics,
        })))
    }

    #[must_use]
    pub const fn unavailable(phase: Option<Phase>, reason: Reason) -> Self {
        Self::Unavailable(UnavailableScalarMeasurement { phase, reason })
    }

    #[must_use]
    pub const fn phase(&self) -> Option<&Phase> {
        match self {
            Self::Available(value) => value.phase.as_ref(),
            Self::Unavailable(value) => value.phase.as_ref(),
        }
    }

    #[must_use]
    pub const fn availability(&self) -> Availability {
        match self {
            Self::Available(_) => Availability::Available,
            Self::Unavailable(_) => Availability::Unavailable,
        }
    }

    fn has_status(&self, status: SampleStatus) -> bool {
        match self {
            Self::Available(value) => value.samples.iter().any(|sample| sample.status() == status),
            Self::Unavailable(_) => false,
        }
    }

    const fn has_samples(&self) -> bool {
        matches!(self, Self::Available(_))
    }
}

impl<Q> Serialize for ScalarMeasurement<Q>
where
    Q: ScalarQuantity,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct MeasurementRef<'a, T> {
            unit: Unit,
            availability: Availability,
            #[serde(flatten)]
            fields: &'a T,
        }

        match self {
            Self::Available(fields) => MeasurementRef {
                unit: Q::UNIT,
                availability: Availability::Available,
                fields,
            }
            .serialize(serializer),
            Self::Unavailable(fields) => MeasurementRef {
                unit: Q::UNIT,
                availability: Availability::Unavailable,
                fields,
            }
            .serialize(serializer),
        }
    }
}

impl<'de, Q> Deserialize<'de> for ScalarMeasurement<Q>
where
    Q: ScalarQuantity,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(bound(deserialize = "Q: DeserializeOwned"), deny_unknown_fields)]
        struct AvailableRaw<Q> {
            unit: Unit,
            availability: Availability,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            phase: Option<Phase>,
            samples: Vec<ScalarObservation<Q>>,
            statistics: Statistics<Q>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnavailableRaw {
            unit: Unit,
            availability: Availability,
            #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
            phase: Option<Phase>,
            reason: Reason,
        }

        let value = Value::deserialize(deserializer)?;
        match read_tag::<D::Error>(&value, "availability")?.as_str() {
            "available" => {
                let raw: AvailableRaw<Q> =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                expect_unit::<D::Error>(raw.unit, Q::UNIT)?;
                if raw.availability != Availability::Available {
                    return Err(D::Error::custom("invalid availability"));
                }
                Self::available(raw.phase, raw.samples, raw.statistics).map_err(D::Error::custom)
            }
            "unavailable" => {
                let raw: UnavailableRaw =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                expect_unit::<D::Error>(raw.unit, Q::UNIT)?;
                if raw.availability != Availability::Unavailable {
                    return Err(D::Error::custom("invalid availability"));
                }
                Ok(Self::unavailable(raw.phase, raw.reason))
            }
            _ => Err(D::Error::custom("unknown measurement availability")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProofSizeMeasurement(ScalarMeasurement<ByteSize>);

impl ProofSizeMeasurement {
    /// Constructs a proof-size measurement with a required phase.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::MissingProofSizePhase`] when `measurement`
    /// has no phase.
    pub fn new(measurement: ScalarMeasurement<ByteSize>) -> Result<Self, MeasurementError> {
        if measurement.phase().is_some() {
            Ok(Self(measurement))
        } else {
            Err(MeasurementError::MissingProofSizePhase)
        }
    }

    const fn measurement(&self) -> &ScalarMeasurement<ByteSize> {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProofSizeMeasurement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(ScalarMeasurement::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    Duration(DurationMeasurement),
    PeakResidentMemory(ScalarMeasurement<ByteSize>),
    ProofSize(ProofSizeMeasurement),
    ProvingKeySize(ScalarMeasurement<ByteSize>),
    VerificationKeySize(ScalarMeasurement<ByteSize>),
    ConstraintCount(ScalarMeasurement<Count>),
    CycleCount(ScalarMeasurement<Count>),
}

impl Measurement {
    #[must_use]
    pub const fn duration(measurement: DurationMeasurement) -> Self {
        Self::Duration(measurement)
    }

    #[must_use]
    pub const fn peak_resident_memory(measurement: ScalarMeasurement<ByteSize>) -> Self {
        Self::PeakResidentMemory(measurement)
    }

    /// Constructs a proof-size metric with a required phase.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementError::MissingProofSizePhase`] when `measurement`
    /// has no phase.
    pub fn proof_size(measurement: ScalarMeasurement<ByteSize>) -> Result<Self, MeasurementError> {
        ProofSizeMeasurement::new(measurement).map(Self::ProofSize)
    }

    #[must_use]
    pub const fn proving_key_size(measurement: ScalarMeasurement<ByteSize>) -> Self {
        Self::ProvingKeySize(measurement)
    }

    #[must_use]
    pub const fn verification_key_size(measurement: ScalarMeasurement<ByteSize>) -> Self {
        Self::VerificationKeySize(measurement)
    }

    #[must_use]
    pub const fn constraint_count(measurement: ScalarMeasurement<Count>) -> Self {
        Self::ConstraintCount(measurement)
    }

    #[must_use]
    pub const fn cycle_count(measurement: ScalarMeasurement<Count>) -> Self {
        Self::CycleCount(measurement)
    }

    #[must_use]
    pub const fn metric(&self) -> Metric {
        match self {
            Self::Duration(_) => Metric::Duration,
            Self::PeakResidentMemory(_) => Metric::PeakResidentMemory,
            Self::ProofSize(_) => Metric::ProofSize,
            Self::ProvingKeySize(_) => Metric::ProvingKeySize,
            Self::VerificationKeySize(_) => Metric::VerificationKeySize,
            Self::ConstraintCount(_) => Metric::ConstraintCount,
            Self::CycleCount(_) => Metric::CycleCount,
        }
    }

    #[must_use]
    pub const fn unit(&self) -> Unit {
        match self {
            Self::Duration(_) => Unit::Nanosecond,
            Self::PeakResidentMemory(_)
            | Self::ProofSize(_)
            | Self::ProvingKeySize(_)
            | Self::VerificationKeySize(_) => Unit::Byte,
            Self::ConstraintCount(_) | Self::CycleCount(_) => Unit::Count,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> Option<&Phase> {
        match self {
            Self::Duration(value) => Some(value.phase()),
            Self::PeakResidentMemory(value)
            | Self::ProvingKeySize(value)
            | Self::VerificationKeySize(value) => value.phase(),
            Self::ProofSize(value) => value.measurement().phase(),
            Self::ConstraintCount(value) | Self::CycleCount(value) => value.phase(),
        }
    }

    #[must_use]
    pub const fn availability(&self) -> Availability {
        match self {
            Self::Duration(value) => value.availability(),
            Self::PeakResidentMemory(value)
            | Self::ProvingKeySize(value)
            | Self::VerificationKeySize(value) => value.availability(),
            Self::ProofSize(value) => value.measurement().availability(),
            Self::ConstraintCount(value) | Self::CycleCount(value) => value.availability(),
        }
    }

    #[must_use]
    pub fn has_sample_status(&self, status: SampleStatus) -> bool {
        match self {
            Self::Duration(value) => value.has_status(status),
            Self::PeakResidentMemory(value)
            | Self::ProvingKeySize(value)
            | Self::VerificationKeySize(value) => value.has_status(status),
            Self::ProofSize(value) => value.measurement().has_status(status),
            Self::ConstraintCount(value) | Self::CycleCount(value) => value.has_status(status),
        }
    }

    #[must_use]
    pub const fn has_samples(&self) -> bool {
        match self {
            Self::Duration(value) => value.has_samples(),
            Self::PeakResidentMemory(value)
            | Self::ProvingKeySize(value)
            | Self::VerificationKeySize(value) => value.has_samples(),
            Self::ProofSize(value) => value.measurement().has_samples(),
            Self::ConstraintCount(value) | Self::CycleCount(value) => value.has_samples(),
        }
    }

    pub(crate) fn validate_correctness(
        &self,
        benchmark: &BenchmarkMetadata,
    ) -> Result<(), MeasurementError> {
        if let Self::Duration(value) = self {
            value.validate_correctness(benchmark)?;
        }
        Ok(())
    }

    pub(crate) fn observations(&self) -> Vec<ObservationView<'_>> {
        match self {
            Self::Duration(DurationMeasurement::Available(value)) => value
                .samples
                .iter()
                .map(duration_observation_view)
                .collect(),
            Self::PeakResidentMemory(ScalarMeasurement::Available(value))
            | Self::ProvingKeySize(ScalarMeasurement::Available(value))
            | Self::VerificationKeySize(ScalarMeasurement::Available(value))
            | Self::ProofSize(ProofSizeMeasurement(ScalarMeasurement::Available(value))) => {
                value.samples.iter().map(scalar_observation_view).collect()
            }
            Self::ConstraintCount(ScalarMeasurement::Available(value))
            | Self::CycleCount(ScalarMeasurement::Available(value)) => {
                value.samples.iter().map(scalar_observation_view).collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn statistics(&self) -> Option<StatisticsView<'_>> {
        match self {
            Self::Duration(DurationMeasurement::Available(value)) => {
                Some(statistics_view(&value.statistics))
            }
            Self::PeakResidentMemory(ScalarMeasurement::Available(value))
            | Self::ProvingKeySize(ScalarMeasurement::Available(value))
            | Self::VerificationKeySize(ScalarMeasurement::Available(value))
            | Self::ProofSize(ProofSizeMeasurement(ScalarMeasurement::Available(value))) => {
                Some(statistics_view(&value.statistics))
            }
            Self::ConstraintCount(ScalarMeasurement::Available(value))
            | Self::CycleCount(ScalarMeasurement::Available(value)) => {
                Some(statistics_view(&value.statistics))
            }
            _ => None,
        }
    }

    pub(crate) const fn duration_policy_counts(&self) -> Option<(u64, u64)> {
        match self {
            Self::Duration(DurationMeasurement::Available(value)) => Some((
                value.policy.warmup_count,
                value.policy.measured_trial_count.get(),
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ObservationView<'a> {
    pub identity: &'a crate::SampleIdentity,
    pub status: SampleStatus,
    pub value: Option<u64>,
    pub error_code: Option<&'a crate::Slug>,
    pub reason: Option<&'a Reason>,
}

pub(crate) enum StatisticsView<'a> {
    Available {
        sample_count: u64,
        minimum: f64,
        maximum: f64,
        median: f64,
        mean: Option<f64>,
        standard_deviation: Option<f64>,
        percentiles: Vec<(&'a str, f64)>,
        included_attempt_ids: &'a [AttemptId],
        excluded_warmup_attempt_ids: &'a [AttemptId],
        flagged_outlier_attempt_ids: &'a [AttemptId],
        status_counts: &'a StatusCounts,
        failure_rate: f64,
        timeout_rate: f64,
    },
    Unavailable {
        status_counts: &'a StatusCounts,
        failure_rate: f64,
        timeout_rate: f64,
    },
}

fn duration_observation_view(sample: &DurationObservation) -> ObservationView<'_> {
    use crate::DurationOutcome;

    let (value, error_code, reason) = match sample.outcome() {
        DurationOutcome::Success { timing, .. } => (Some(timing.duration().get()), None, None),
        DurationOutcome::Unsupported { reason } => (None, None, Some(reason)),
        DurationOutcome::Failed {
            error_code, reason, ..
        } => (None, Some(error_code), Some(reason)),
        DurationOutcome::TimedOut {
            error_code, reason, ..
        } => (None, error_code.as_ref(), reason.as_ref()),
        DurationOutcome::Invalid {
            error_code, reason, ..
        } => (None, error_code.as_ref(), Some(reason)),
    };
    ObservationView {
        identity: sample.identity(),
        status: sample.status(),
        value,
        error_code,
        reason,
    }
}

fn scalar_observation_view<Q>(sample: &ScalarObservation<Q>) -> ObservationView<'_>
where
    Q: ScalarQuantity,
{
    use crate::ScalarOutcome;

    let (value, error_code, reason) = match sample.outcome() {
        ScalarOutcome::Success { value } => (Some(value.canonical_value()), None, None),
        ScalarOutcome::Unsupported { reason } => (None, None, Some(reason)),
        ScalarOutcome::Failed { error_code, reason } => (None, Some(error_code), Some(reason)),
        ScalarOutcome::TimedOut { error_code, reason } => {
            (None, error_code.as_ref(), reason.as_ref())
        }
        ScalarOutcome::Invalid { error_code, reason } => (None, error_code.as_ref(), Some(reason)),
    };
    ObservationView {
        identity: sample.identity(),
        status: sample.status(),
        value,
        error_code,
        reason,
    }
}

fn statistics_view<Q>(statistics: &Statistics<Q>) -> StatisticsView<'_> {
    match statistics {
        Statistics::Available(value) => StatisticsView::Available {
            sample_count: value.sample_count,
            minimum: value.minimum.as_f64(),
            maximum: value.maximum.as_f64(),
            median: value.median.as_f64(),
            mean: value.mean.as_ref().map(SummaryValue::as_f64),
            standard_deviation: value.standard_deviation.as_ref().map(SummaryValue::as_f64),
            percentiles: value
                .percentiles
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_f64()))
                .collect(),
            included_attempt_ids: &value.included_attempt_ids,
            excluded_warmup_attempt_ids: &value.excluded_warmup_attempt_ids,
            flagged_outlier_attempt_ids: &value.flagged_outlier_attempt_ids,
            status_counts: &value.status_counts,
            failure_rate: value.rates.failure.as_f64(),
            timeout_rate: value.rates.timeout.as_f64(),
        },
        Statistics::Unavailable(value) => StatisticsView::Unavailable {
            status_counts: &value.status_counts,
            failure_rate: value.rates.failure.as_f64(),
            timeout_rate: value.rates.timeout.as_f64(),
        },
    }
}

impl Serialize for Measurement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct TaggedRef<'a, T> {
            metric: Metric,
            #[serde(flatten)]
            measurement: &'a T,
        }

        match self {
            Self::Duration(value) => TaggedRef {
                metric: Metric::Duration,
                measurement: value,
            }
            .serialize(serializer),
            Self::PeakResidentMemory(value) => TaggedRef {
                metric: Metric::PeakResidentMemory,
                measurement: value,
            }
            .serialize(serializer),
            Self::ProofSize(value) => TaggedRef {
                metric: Metric::ProofSize,
                measurement: value,
            }
            .serialize(serializer),
            Self::ProvingKeySize(value) => TaggedRef {
                metric: Metric::ProvingKeySize,
                measurement: value,
            }
            .serialize(serializer),
            Self::VerificationKeySize(value) => TaggedRef {
                metric: Metric::VerificationKeySize,
                measurement: value,
            }
            .serialize(serializer),
            Self::ConstraintCount(value) => TaggedRef {
                metric: Metric::ConstraintCount,
                measurement: value,
            }
            .serialize(serializer),
            Self::CycleCount(value) => TaggedRef {
                metric: Metric::CycleCount,
                measurement: value,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Measurement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let metric = take_tag::<D::Error>(&mut value, "metric")?;
        match metric.as_str() {
            "duration" => serde_json::from_value(value)
                .map(Self::Duration)
                .map_err(D::Error::custom),
            "peak_resident_memory" => serde_json::from_value(value)
                .map(Self::PeakResidentMemory)
                .map_err(D::Error::custom),
            "proof_size" => {
                let measurement: ProofSizeMeasurement =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                Ok(Self::ProofSize(measurement))
            }
            "proving_key_size" => serde_json::from_value(value)
                .map(Self::ProvingKeySize)
                .map_err(D::Error::custom),
            "verification_key_size" => serde_json::from_value(value)
                .map(Self::VerificationKeySize)
                .map_err(D::Error::custom),
            "constraint_count" => serde_json::from_value(value)
                .map(Self::ConstraintCount)
                .map_err(D::Error::custom),
            "cycle_count" => serde_json::from_value(value)
                .map(Self::CycleCount)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom("unknown BenchmarkReport metric")),
        }
    }
}

fn valid_percentile(key: &str) -> bool {
    let Some(number) = key.strip_prefix('p') else {
        return false;
    };
    if number == "100" {
        return true;
    }
    let (integer, fraction) = number
        .split_once('.')
        .map_or((number, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if integer.is_empty()
        || integer.len() > 2
        || (integer.len() > 1 && integer.starts_with('0'))
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    fraction
        .is_none_or(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
}

fn expect_unit<E>(actual: Unit, expected: Unit) -> Result<(), E>
where
    E: serde::de::Error,
{
    if actual == expected {
        Ok(())
    } else {
        Err(E::custom(MeasurementError::InvalidUnit {
            expected,
            actual,
        }))
    }
}

fn read_tag<E>(value: &Value, name: &str) -> Result<String, E>
where
    E: serde::de::Error,
{
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| E::custom(format!("missing string field {name}")))
}

fn take_tag<E>(value: &mut Value, name: &str) -> Result<String, E>
where
    E: serde::de::Error,
{
    value
        .as_object_mut()
        .and_then(|object| object.remove(name))
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| E::custom(format!("missing string field {name}")))
}
