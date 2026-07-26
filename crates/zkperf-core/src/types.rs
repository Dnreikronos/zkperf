use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use chrono::{DateTime, FixedOffset};
use semver::Version;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Number;
use uuid::Uuid;

#[derive(Eq, PartialEq)]
struct ExactDecimal {
    negative: bool,
    digits: String,
    exponent: i64,
}

impl ExactDecimal {
    fn from_number(value: &Number) -> Self {
        let encoded = value.to_string();
        let (negative, unsigned) = encoded
            .strip_prefix('-')
            .map_or((false, encoded.as_str()), |unsigned| (true, unsigned));
        let (significand, exponent) = unsigned.find(['e', 'E']).map_or((unsigned, 0), |index| {
            (
                &unsigned[..index],
                parse_decimal_exponent(&unsigned[index + 1..]),
            )
        });
        let fractional_digits = significand
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        let exponent =
            exponent.saturating_sub(i64::try_from(fractional_digits).unwrap_or(i64::MAX));
        let digits = significand.replace('.', "");
        Self::normalize(negative, &digits, exponent)
    }

    fn normalize(negative: bool, digits: &str, exponent: i64) -> Self {
        let Some(first_nonzero) = digits.bytes().position(|digit| digit != b'0') else {
            return Self::zero();
        };
        let trailing_zeros = digits
            .bytes()
            .rev()
            .take_while(|digit| *digit == b'0')
            .count();
        let end = digits.len() - trailing_zeros;
        Self {
            negative,
            digits: digits[first_nonzero..end].to_owned(),
            exponent: exponent.saturating_add(i64::try_from(trailing_zeros).unwrap_or(i64::MAX)),
        }
    }

    fn zero() -> Self {
        Self {
            negative: false,
            digits: String::new(),
            exponent: 0,
        }
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    fn is_at_most_one(&self) -> bool {
        if self.negative {
            return false;
        }
        if self.is_zero() {
            return true;
        }
        let order =
            i128::try_from(self.digits.len()).unwrap_or(i128::MAX) + i128::from(self.exponent);
        order < 1 || (order == 1 && self.digits == "1" && self.exponent == 0)
    }
}

fn parse_decimal_exponent(value: &str) -> i64 {
    let (negative, digits) = value
        .strip_prefix('-')
        .map_or((false, value), |digits| (true, digits));
    let digits = digits.strip_prefix('+').unwrap_or(digits);
    let magnitude = digits.bytes().fold(0_i64, |magnitude, digit| {
        magnitude
            .saturating_mul(10)
            .saturating_add(i64::from(digit - b'0'))
    });
    if negative {
        magnitude.saturating_neg()
    } else {
        magnitude
    }
}

pub(crate) fn number_is_nonnegative(value: &Number) -> bool {
    let decimal = ExactDecimal::from_number(value);
    !decimal.negative || decimal.is_zero()
}

pub(crate) fn number_is_unit_interval(value: &Number) -> bool {
    ExactDecimal::from_number(value).is_at_most_one()
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DomainError {
    EmptyValue,
    InvalidSlug(String),
    InvalidSemanticVersion(String),
    InvalidTimestamp(String),
    StopBeforeStart,
    DurationMismatch {
        expected: Nanoseconds,
        actual: Nanoseconds,
    },
    TooFewCombinedPhases,
    DuplicateCombinedPhase(ComponentPhase),
}

impl Display for DomainError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue => formatter.write_str("value must not be empty"),
            Self::InvalidSlug(value) => write!(formatter, "invalid slug: {value}"),
            Self::InvalidSemanticVersion(value) => {
                write!(formatter, "invalid semantic version: {value}")
            }
            Self::InvalidTimestamp(value) => {
                write!(formatter, "invalid RFC 3339 timestamp: {value}")
            }
            Self::StopBeforeStart => formatter.write_str("stop time must not precede start time"),
            Self::DurationMismatch { expected, actual } => write!(
                formatter,
                "duration does not match stop - start: expected {}, got {}",
                expected.get(),
                actual.get()
            ),
            Self::TooFewCombinedPhases => {
                formatter.write_str("a combined phase requires at least two components")
            }
            Self::DuplicateCombinedPhase(phase) => {
                write!(
                    formatter,
                    "combined phase contains duplicate component {phase}"
                )
            }
        }
    }
}

impl Error for DomainError {}

macro_rules! identifier {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub const fn new(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> Uuid {
                self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                Display::fmt(&self.0, formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

identifier!(ReportId);
identifier!(RunId);
identifier!(SampleId);
identifier!(AttemptId);
identifier!(ArtifactId);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Creates a string that is known to contain at least one byte.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::EmptyValue`] when `value` is empty.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        Self::try_from(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(DomainError::EmptyValue)
        } else {
            Ok(Self(value))
        }
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Slug(String);

impl Slug {
    /// Creates a lowercase identifier accepted by `BenchmarkReport` v1.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSlug`] when `value` does not match the
    /// report schema's slug grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        Self::try_from(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Slug {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut previous_was_separator = true;
        let valid = !value.is_empty()
            && value.bytes().all(|byte| {
                if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
                    previous_was_separator = false;
                    true
                } else if matches!(byte, b'.' | b'_' | b'-') && !previous_was_separator {
                    previous_was_separator = true;
                    true
                } else {
                    false
                }
            })
            && !previous_was_separator;

        if valid {
            Ok(Self(value))
        } else {
            Err(DomainError::InvalidSlug(value))
        }
    }
}

impl<'de> Deserialize<'de> for Slug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SemanticVersion(Version);

impl SemanticVersion {
    /// Parses a semantic version used by compatibility metadata.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSemanticVersion`] when `value` is not a
    /// valid Semantic Versioning version.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Version::parse(value)
            .map(Self)
            .map_err(|_| DomainError::InvalidSemanticVersion(value.to_owned()))
    }

    #[must_use]
    pub fn get(&self) -> &Version {
        &self.0
    }
}

impl Display for SemanticVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl Serialize for SemanticVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SemanticVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SchemaVersion {
    V1_0_0,
}

impl SchemaVersion {
    pub const V1: &str = "1.0.0";

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1_0_0 => Self::V1,
        }
    }
}

impl Display for SchemaVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            Self::V1 => Ok(Self::V1_0_0),
            version => Err(D::Error::custom(format!(
                "unsupported BenchmarkReport schema version {version}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Timestamp {
    encoded: String,
    value: DateTime<FixedOffset>,
}

impl Timestamp {
    /// Parses and retains an RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidTimestamp`] when `value` is not RFC 3339.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let encoded = value.into();
        let value = DateTime::parse_from_rfc3339(&encoded)
            .map_err(|_| DomainError::InvalidTimestamp(encoded.clone()))?;
        Ok(Self { encoded, value })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    #[must_use]
    pub fn precedes_or_equals(&self, other: &Self) -> bool {
        self.value <= other.value
    }
}

impl PartialEq for Timestamp {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for Timestamp {}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.encoded)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

macro_rules! quantity {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }
    };
}

quantity!(Nanoseconds);
quantity!(ByteSize);
quantity!(Count);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Timing {
    #[serde(rename = "start_ns")]
    start: Nanoseconds,
    #[serde(rename = "stop_ns")]
    stop: Nanoseconds,
    #[serde(rename = "duration_ns")]
    duration: Nanoseconds,
}

impl Timing {
    /// Constructs timing from monotonic-clock boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StopBeforeStart`] when `stop_ns` precedes
    /// `start_ns`.
    pub fn new(start_ns: Nanoseconds, stop_ns: Nanoseconds) -> Result<Self, DomainError> {
        let duration = stop_ns
            .get()
            .checked_sub(start_ns.get())
            .ok_or(DomainError::StopBeforeStart)?;
        Ok(Self {
            start: start_ns,
            stop: stop_ns,
            duration: Nanoseconds::new(duration),
        })
    }

    /// Constructs timing while checking a producer-supplied duration.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StopBeforeStart`] for reversed boundaries or
    /// [`DomainError::DurationMismatch`] when `duration_ns` is not
    /// `stop_ns - start_ns`.
    pub fn from_parts(
        start_ns: Nanoseconds,
        stop_ns: Nanoseconds,
        duration_ns: Nanoseconds,
    ) -> Result<Self, DomainError> {
        let timing = Self::new(start_ns, stop_ns)?;
        if timing.duration == duration_ns {
            Ok(timing)
        } else {
            Err(DomainError::DurationMismatch {
                expected: timing.duration,
                actual: duration_ns,
            })
        }
    }

    #[must_use]
    pub const fn start(self) -> Nanoseconds {
        self.start
    }

    #[must_use]
    pub const fn stop(self) -> Nanoseconds {
        self.stop
    }

    #[must_use]
    pub const fn duration(self) -> Nanoseconds {
        self.duration
    }
}

impl<'de> Deserialize<'de> for Timing {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawTiming {
            #[serde(rename = "start_ns")]
            start: Nanoseconds,
            #[serde(rename = "stop_ns")]
            stop: Nanoseconds,
            #[serde(rename = "duration_ns")]
            duration: Nanoseconds,
        }

        let raw = RawTiming::deserialize(deserializer)?;
        Self::from_parts(raw.start, raw.stop, raw.duration).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardPhase {
    Build,
    Setup,
    Execution,
    Proving,
    Compression,
    Verification,
    EndToEnd,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentPhase {
    Build,
    Setup,
    Execution,
    Proving,
    Compression,
    Verification,
}

impl Display for ComponentPhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Build => "build",
            Self::Setup => "setup",
            Self::Execution => "execution",
            Self::Proving => "proving",
            Self::Compression => "compression",
            Self::Verification => "verification",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CombinedPhase(Vec<ComponentPhase>);

impl CombinedPhase {
    /// Creates an ordered combined phase.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::TooFewCombinedPhases`] for fewer than two
    /// components and [`DomainError::DuplicateCombinedPhase`] for duplicates.
    pub fn new(components: Vec<ComponentPhase>) -> Result<Self, DomainError> {
        if components.len() < 2 {
            return Err(DomainError::TooFewCombinedPhases);
        }

        let mut unique = HashSet::with_capacity(components.len());
        for component in &components {
            if !unique.insert(*component) {
                return Err(DomainError::DuplicateCombinedPhase(*component));
            }
        }

        Ok(Self(components))
    }

    #[must_use]
    pub fn components(&self) -> &[ComponentPhase] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    Standard(StandardPhase),
    Combined(CombinedPhase),
}

impl Phase {
    #[must_use]
    pub const fn standard(name: StandardPhase) -> Self {
        Self::Standard(name)
    }

    /// Creates a combined phase from ordered, distinct components.
    ///
    /// # Errors
    ///
    /// Returns the validation error from [`CombinedPhase::new`].
    pub fn combined(components: Vec<ComponentPhase>) -> Result<Self, DomainError> {
        CombinedPhase::new(components).map(Self::Combined)
    }

    #[must_use]
    pub fn is_proof_related(&self) -> bool {
        match self {
            Self::Standard(phase) => matches!(
                phase,
                StandardPhase::Proving
                    | StandardPhase::Compression
                    | StandardPhase::Verification
                    | StandardPhase::EndToEnd
            ),
            Self::Combined(phase) => phase.components().iter().any(|component| {
                matches!(
                    component,
                    ComponentPhase::Proving
                        | ComponentPhase::Compression
                        | ComponentPhase::Verification
                )
            }),
        }
    }

    #[must_use]
    pub fn is_execution_only(&self) -> bool {
        match self {
            Self::Standard(phase) => *phase == StandardPhase::Execution,
            Self::Combined(phase) => {
                let components = phase.components();
                components.contains(&ComponentPhase::Execution) && !self.is_proof_related()
            }
        }
    }
}

impl Serialize for Phase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum RawPhase<'a> {
            Standard { name: StandardPhase },
            Combined { components: &'a [ComponentPhase] },
        }

        match self {
            Self::Standard(name) => RawPhase::Standard { name: *name }.serialize(serializer),
            Self::Combined(combined) => RawPhase::Combined {
                components: combined.components(),
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Phase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum RawPhase {
            Standard { name: StandardPhase },
            Combined { components: Vec<ComponentPhase> },
        }

        match RawPhase::deserialize(deserializer)? {
            RawPhase::Standard { name } => Ok(Self::Standard(name)),
            RawPhase::Combined { components } => {
                Self::combined(components).map_err(D::Error::custom)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ByteSize, ComponentPhase, Count, DomainError, Nanoseconds, Phase, SchemaVersion, Slug,
        Timestamp, Timing,
    };

    #[test]
    fn quantities_keep_their_canonical_units_distinct() {
        let duration = Nanoseconds::new(42);
        let bytes = ByteSize::new(42);
        let count = Count::new(42);

        assert_eq!(duration.get(), bytes.get());
        assert_eq!(bytes.get(), count.get());
        assert_eq!(serde_json::to_string(&duration).unwrap(), "42");
    }

    #[test]
    fn timing_rejects_inconsistent_arithmetic() {
        let error =
            serde_json::from_str::<Timing>(r#"{"start_ns":10,"stop_ns":15,"duration_ns":6}"#)
                .unwrap_err();

        assert!(error.to_string().contains("expected 5, got 6"));
        assert_eq!(
            Timing::new(Nanoseconds::new(2), Nanoseconds::new(1)),
            Err(DomainError::StopBeforeStart)
        );
    }

    #[test]
    fn combined_phases_require_distinct_components() {
        assert!(Phase::combined(vec![ComponentPhase::Execution]).is_err());
        assert!(
            Phase::combined(vec![ComponentPhase::Execution, ComponentPhase::Execution]).is_err()
        );
    }

    #[test]
    fn validated_strings_and_versions_reject_invalid_input() {
        assert!(Slug::new("not a slug").is_err());
        assert!(Timestamp::parse("yesterday").is_err());
        assert!(serde_json::from_str::<SchemaVersion>(r#""2.0.0""#).is_err());
    }
}
