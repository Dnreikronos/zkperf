//! Stable host and harness evidence, captured before adapter startup.

mod host;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ClockMetadata, HostMetadata, NonEmptyString, Observed, Sha256Digest};

/// Immutable input to the environment digest. It contains no run identity or time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentCapture {
    pub capture_version: crate::SchemaVersion,
    pub host: HostMetadata,
    pub clock: ClockMetadata,
    pub harness: HarnessBuild,
    pub environment_variables: BTreeMap<String, Observed<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessBuild {
    pub version: NonEmptyString,
    pub rustc: Observed<NonEmptyString>,
    pub target: Observed<NonEmptyString>,
    pub profile: Observed<NonEmptyString>,
    pub opt_level: Observed<NonEmptyString>,
    pub debug: Observed<NonEmptyString>,
    pub source_revision: Observed<NonEmptyString>,
    pub compiler_flags: Observed<Vec<String>>,
    pub lockfile_digest: Sha256Digest,
    pub executable_digest: Observed<Sha256Digest>,
}

impl EnvironmentCapture {
    /// Collects static metadata without inheriting or enumerating environment variables.
    ///
    /// # Panics
    /// Panics if a built-in metadata constant violates its domain constraints.
    #[must_use]
    pub fn collect(environment: &BTreeMap<String, String>) -> Self {
        let executable_digest = std::env::current_exe()
            .ok()
            .and_then(|path| crate::digest::hash_file(&path).ok())
            .map_or_else(
                || Observed::gap("not_readable", "Harness executable could not be hashed."),
                |(digest, _)| digest.into(),
            );
        Self {
            capture_version: crate::SchemaVersion::V1_0_0,
            host: host::collect(),
            clock: ClockMetadata::new(
                NonEmptyString::new("std::time::Instant").unwrap(),
                Observed::gap(
                    "not_exposed",
                    "Rust Instant does not expose the clock resolution.",
                ),
            ),
            harness: HarnessBuild {
                version: NonEmptyString::new(env!("CARGO_PKG_VERSION")).unwrap(),
                rustc: host::text(option_env!("ZKPERF_BUILD_RUSTC")),
                target: host::text(option_env!("ZKPERF_BUILD_TARGET")),
                profile: host::text(option_env!("ZKPERF_BUILD_PROFILE")),
                opt_level: host::text(option_env!("ZKPERF_BUILD_OPT_LEVEL")),
                debug: host::text(option_env!("ZKPERF_BUILD_DEBUG")),
                source_revision: Observed::gap(
                    "not_embedded",
                    "Source revision was not embedded in this build.",
                ),
                compiler_flags: Observed::gap(
                    "excluded",
                    "Arbitrary compiler flags may contain private paths or secrets.",
                ),
                lockfile_digest: Sha256Digest::from_bytes(crate::digest::hash_bytes(
                    include_bytes!("../../../Cargo.lock"),
                )),
                executable_digest,
            },
            environment_variables: safe_environment(environment),
        }
    }

    /// Hashes compact, deterministic JSON of this capture, including gap reasons.
    ///
    /// # Panics
    /// Panics if the metadata types cannot be serialized as JSON.
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::from_bytes(crate::digest::hash_bytes(
            &serde_json::to_vec(self).expect("environment metadata serializes"),
        ))
    }
}

fn safe_environment(environment: &BTreeMap<String, String>) -> BTreeMap<String, Observed<String>> {
    environment
        .iter()
        .filter_map(|(name, value)| {
            let safe = match name.as_str() {
                "RAYON_NUM_THREADS"
                | "OMP_NUM_THREADS"
                | "OPENBLAS_NUM_THREADS"
                | "MKL_NUM_THREADS"
                | "NUMEXPR_NUM_THREADS" => {
                    !value.is_empty()
                        && value.bytes().all(|byte| byte.is_ascii_digit())
                        && value.parse::<u32>().is_ok_and(|value| value > 0)
                }
                "OMP_DYNAMIC" | "OMP_NESTED" => {
                    matches!(value.as_str(), "true" | "false" | "TRUE" | "FALSE")
                }
                _ => return None,
            };
            Some((
                name.clone(),
                if safe {
                    value.clone().into()
                } else {
                    Observed::gap(
                        "excluded",
                        "Value does not satisfy the safe environment grammar.",
                    )
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
