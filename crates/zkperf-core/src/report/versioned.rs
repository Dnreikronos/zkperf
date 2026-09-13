use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    BenchmarkReportV1Parts, Measurement, ReportData, ReportError, ReportStatus, SchemaVersion,
};

/// Report 1.0.0 requires known values for all required environment fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BenchmarkReportV1(ReportData);

/// Report 2.0.0 permits explicit unavailable environment metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BenchmarkReportV2(ReportData);

/// Both report versions use the same inputs; v1 rejects unavailable metadata.
pub type BenchmarkReportV2Parts = BenchmarkReportV1Parts;

impl BenchmarkReportV1 {
    /// Constructs a report under the unchanged 1.0.0 contract.
    ///
    /// # Errors
    /// Returns [`ReportError`] for unavailable environment metadata or invalid
    /// measurements, security settings, or outcomes.
    pub fn new(parts: BenchmarkReportV1Parts) -> Result<Self, ReportError> {
        ReportData::new(parts, SchemaVersion::V1_0_0).map(Self)
    }

    #[must_use]
    pub fn measurements(&self) -> &[Measurement] {
        &self.0.measurements
    }

    #[must_use]
    pub const fn status(&self) -> &ReportStatus {
        &self.0.status
    }
}

impl BenchmarkReportV2 {
    /// Constructs a report that can preserve unavailable environment metadata.
    ///
    /// # Errors
    /// Returns [`ReportError`] for invalid measurements, security settings, or outcomes.
    pub fn new(parts: BenchmarkReportV2Parts) -> Result<Self, ReportError> {
        ReportData::new(parts, SchemaVersion::V2_0_0).map(Self)
    }

    #[must_use]
    pub fn measurements(&self) -> &[Measurement] {
        &self.0.measurements
    }

    #[must_use]
    pub const fn status(&self) -> &ReportStatus {
        &self.0.status
    }
}

impl<'de> Deserialize<'de> for BenchmarkReportV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = ReportData::deserialize(deserializer)?;
        if data.schema_version != SchemaVersion::V1_0_0 {
            return Err(D::Error::custom("expected report schema 1.0.0"));
        }
        Ok(Self(data))
    }
}

impl<'de> Deserialize<'de> for BenchmarkReportV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = ReportData::deserialize(deserializer)?;
        if data.schema_version != SchemaVersion::V2_0_0 {
            return Err(D::Error::custom("expected report schema 2.0.0"));
        }
        Ok(Self(data))
    }
}
