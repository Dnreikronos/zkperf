//! Deterministic, auditable schedules built before adapter execution.

mod provenance;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{BenchmarkManifest, FixtureHashError, ManifestEngine, Slug};
pub use provenance::FileProvenance;

/// A complete schedule and the immutable effective definition that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkPlan {
    id: String,
    manifest: BenchmarkManifest,
    files: Vec<FileProvenance>,
    jobs: Vec<PlannedJob>,
}

impl BenchmarkPlan {
    /// Expands a validated manifest without discovering or executing adapters.
    ///
    /// # Errors
    /// Returns an error if counts overflow, allocation fails, a referenced file
    /// cannot be hashed, or the effective definition cannot be serialized.
    pub fn build(manifest: BenchmarkManifest) -> Result<Self, PlanError> {
        let count = job_count(&manifest).ok_or(PlanError::TooManyJobs)?;
        let mut jobs = Vec::new();
        jobs.try_reserve_exact(count)
            .map_err(|_| PlanError::TooManyJobs)?;
        let files = provenance::collect(&manifest)?;
        let definition = serde_json::to_vec(&("zkperf-plan-v1", &manifest, &files))?;
        let id = crate::digest::encode_hex(&crate::digest::hash_bytes(&definition));
        let engines = ordered_engines(&manifest);
        for (warmup, repetitions) in [
            (true, manifest.run().warmups()),
            (false, manifest.run().runs().get()),
        ] {
            for workload in manifest.workloads() {
                for input in workload.inputs() {
                    for repetition in 0..repetitions {
                        let offset = usize::try_from(repetition % engines.len() as u64)
                            .map_err(|_| PlanError::TooManyJobs)?;
                        for engine in engines.iter().cycle().skip(offset).take(engines.len()) {
                            for mode_index in 0..engine.proof_modes().len().max(1) {
                                let position = jobs.len() as u64;
                                jobs.push(PlannedJob {
                                    id: format!("{id}:{position}"),
                                    position,
                                    engine_id: engine.id().clone(),
                                    workload_id: workload.id().clone(),
                                    input_id: input.id().clone(),
                                    proof_mode: engine.proof_modes().get(mode_index).cloned(),
                                    repetition,
                                    warmup,
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(Self {
            id,
            manifest,
            files,
            jobs,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn manifest(&self) -> &BenchmarkManifest {
        &self.manifest
    }

    #[must_use]
    pub fn files(&self) -> &[FileProvenance] {
        &self.files
    }

    #[must_use]
    pub fn jobs(&self) -> &[PlannedJob] {
        &self.jobs
    }

    /// Prints the complete schedule with the manifest's diagnostic redaction.
    ///
    /// # Errors
    /// Returns an error if the plan cannot be represented as JSON.
    pub fn normalized_debug(&self) -> Result<String, serde_json::Error> {
        let manifest: serde_json::Value = serde_json::from_str(&self.manifest.normalized_debug()?)?;
        serde_json::to_string_pretty(&serde_json::json!({
            "plan_version": "1.0.0",
            "id": self.id,
            "manifest": manifest,
            "files": self.files,
            "jobs": self.jobs,
        }))
    }
}

/// One repetition of the workload's phase sequence, bound to plan provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedJob {
    id: String,
    position: u64,
    engine_id: Slug,
    workload_id: Slug,
    input_id: Slug,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_mode: Option<Slug>,
    repetition: u64,
    warmup: bool,
}

impl PlannedJob {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    #[must_use]
    pub const fn engine_id(&self) -> &Slug {
        &self.engine_id
    }

    #[must_use]
    pub const fn workload_id(&self) -> &Slug {
        &self.workload_id
    }

    #[must_use]
    pub const fn input_id(&self) -> &Slug {
        &self.input_id
    }

    #[must_use]
    pub const fn proof_mode(&self) -> Option<&Slug> {
        self.proof_mode.as_ref()
    }

    #[must_use]
    pub const fn repetition(&self) -> u64 {
        self.repetition
    }

    #[must_use]
    pub const fn warmup(&self) -> bool {
        self.warmup
    }
}

fn job_count(manifest: &BenchmarkManifest) -> Option<usize> {
    let repetitions = manifest
        .run()
        .warmups()
        .checked_add(manifest.run().runs().get())?;
    let inputs = manifest
        .workloads()
        .iter()
        .try_fold(0_u64, |total, workload| {
            total.checked_add(workload.inputs().len() as u64)
        })?;
    let modes = manifest.engines().iter().try_fold(0_u64, |total, engine| {
        total.checked_add(engine.proof_modes().len().max(1) as u64)
    })?;
    usize::try_from(repetitions.checked_mul(inputs)?.checked_mul(modes)?).ok()
}

fn ordered_engines(manifest: &BenchmarkManifest) -> Vec<&ManifestEngine> {
    let mut ranked: Vec<_> = manifest
        .engines()
        .iter()
        .map(|engine| {
            let mut hasher = Sha256::new();
            hasher.update(b"zkperf-engine-order-v1");
            hasher.update(manifest.run().policy().seed().to_le_bytes());
            hasher.update(engine.id().as_str().as_bytes());
            (hasher.finalize(), engine)
        })
        .collect();
    ranked.sort_by(|(a, engine_a), (b, engine_b)| {
        a.cmp(b)
            .then_with(|| engine_a.id().as_str().cmp(engine_b.id().as_str()))
    });
    ranked.into_iter().map(|(_, engine)| engine).collect()
}

/// A schedule could not be constructed before execution.
#[derive(Debug)]
pub enum PlanError {
    TooManyJobs,
    Fixture(FixtureHashError),
    Serialization(serde_json::Error),
}

impl Display for PlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyJobs => {
                formatter.write_str("benchmark plan has too many jobs to represent or allocate")
            }
            Self::Fixture(error) => Display::fmt(error, formatter),
            Self::Serialization(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for PlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TooManyJobs => None,
            Self::Fixture(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
}

impl From<FixtureHashError> for PlanError {
    fn from(error: FixtureHashError) -> Self {
        Self::Fixture(error)
    }
}

impl From<serde_json::Error> for PlanError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}
