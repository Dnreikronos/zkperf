use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::num::NonZeroU64;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::{
    BenchmarkManifest, ManifestEngine, ManifestError, ManifestInput, ManifestOutputs, ManifestRun,
    ManifestVersion, ManifestWorkload, OutputFormat, PhaseTimeout, ResolvedFile,
    is_secret_like_key,
};
use crate::{
    Commitment, Concurrency, ConcurrencyMode, ExecutionMode, ImplementationLane, InputVisibility,
    MetadataError, NonEmptyString, RemoteProfile, ResourceLimit, Resources, ResourcesParts,
    RunPolicy, RunPolicyParts, Slug, StandardPhase,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    manifest_version: ManifestVersion,
    run: RawRun,
    outputs: RawOutputs,
    workloads: Vec<RawWorkload>,
    engines: Vec<RawEngine>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    warmups: u64,
    runs: u64,
    policy: RawRunPolicy,
    resources: RawResources,
    timeouts: Vec<RawTimeout>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRunPolicy {
    ordering_algorithm: ManifestOrderingAlgorithm,
    seed: u64,
    concurrency: RawConcurrency,
    retry_policy: ManifestRetryPolicy,
    invalidation_policy: ManifestInvalidationPolicy,
    outlier_rule: ManifestOutlierRule,
    percentile_method: crate::PercentileMethod,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestOrderingAlgorithm {
    SeededRoundRobinV1,
}

impl ManifestOrderingAlgorithm {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SeededRoundRobinV1 => "seeded_round_robin_v1",
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
enum ManifestRetryPolicy {
    #[serde(rename = "no retries except replacements for invalid attempts")]
    NoRetriesExceptInvalidReplacements,
}

impl ManifestRetryPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NoRetriesExceptInvalidReplacements => {
                "no retries except replacements for invalid attempts"
            }
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
enum ManifestInvalidationPolicy {
    #[serde(rename = "replace only verified harness or external interference")]
    ReplaceVerifiedHarnessOrExternalInterference,
}

impl ManifestInvalidationPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReplaceVerifiedHarnessOrExternalInterference => {
                "replace only verified harness or external interference"
            }
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestOutlierRule {
    None,
}

impl ManifestOutlierRule {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConcurrency {
    mode: ConcurrencyMode,
    parallel_attempts: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResources {
    power_profile: NonEmptyString,
    cpu_affinity: Vec<u64>,
    cpu_limit: ResourceLimit,
    memory_limit_bytes: ResourceLimit,
    worker_count: u64,
    accelerator_allocation: Vec<NonEmptyString>,
    environment_variables: BTreeMap<String, String>,
    execution_mode: ExecutionMode,
    network_access: bool,
    #[serde(default, deserialize_with = "crate::deserialize_optional_non_null")]
    remote: Option<RemoteProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTimeout {
    phase: StandardPhase,
    limit_ms: u64,
    termination_grace_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutputs {
    directory: PathBuf,
    formats: Vec<OutputFormat>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkload {
    id: Slug,
    revision: NonEmptyString,
    specification: PathBuf,
    implementation_lane: ImplementationLane,
    security_target_bits: u64,
    phases: Vec<StandardPhase>,
    inputs: Vec<RawInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInput {
    id: Slug,
    fixture: PathBuf,
    expected_output: PathBuf,
    visibility: InputVisibility,
    preprocessing: NonEmptyString,
    commit_input: Commitment,
    commit_output: Commitment,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEngine {
    id: Slug,
    adapter: PathBuf,
    proof_modes: Vec<Slug>,
    #[serde(default)]
    configuration: BTreeMap<String, Value>,
}

pub(super) fn load(path: &Path) -> Result<BenchmarkManifest, ManifestError> {
    let manifest_path = canonical_regular_file(path, "manifest")?;
    let source = fs::read_to_string(&manifest_path).map_err(|error| {
        ManifestError::new(
            "manifest",
            format!("could not read {}: {error}", manifest_path.display()),
        )
    })?;
    let raw = deserialize(&source)?;
    let base = manifest_path
        .parent()
        .ok_or_else(|| ManifestError::new("manifest", "manifest has no parent directory"))?
        .to_path_buf();

    validate(raw, manifest_path, &base)
}

fn deserialize(source: &str) -> Result<RawManifest, ManifestError> {
    let deserializer = toml::Deserializer::parse(source).map_err(|error| {
        let location = error.span().map_or_else(String::new, |span| {
            let (line, column) = line_column(source, span.start);
            format!(" at line {line}, column {column}")
        });
        ManifestError::new("manifest", format!("{}{location}", error.message()))
    })?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let inner = error.inner();
        let field_path = refine_deserialize_path(&error.path().to_string(), inner.message());
        let location = inner.span().map_or_else(String::new, |span| {
            let (line, column) = line_column(source, span.start);
            format!(" at line {line}, column {column}")
        });
        ManifestError::new(field_path, format!("{}{location}", inner.message()))
    })
}

fn refine_deserialize_path(path: &str, message: &str) -> String {
    let mut path = if path.is_empty() || path == "." {
        "manifest".to_owned()
    } else {
        path.to_owned()
    };

    for prefix in ["unknown field `", "missing field `"] {
        let Some(start) = message.find(prefix).map(|index| index + prefix.len()) else {
            continue;
        };
        let Some(end) = message[start..].find('`').map(|index| start + index) else {
            continue;
        };
        let field = &message[start..end];
        if path != field && !path.ends_with(&format!(".{field}")) {
            if path == "manifest" {
                field.clone_into(&mut path);
            } else {
                path.push('.');
                path.push_str(field);
            }
        }
        break;
    }

    path
}

fn line_column(source: &str, byte_offset: usize) -> (usize, usize) {
    let prefix = &source[..byte_offset.min(source.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, trailing)| trailing.len())
        + 1;
    (line, column)
}

fn validate(
    raw: RawManifest,
    manifest_path: PathBuf,
    base: &Path,
) -> Result<BenchmarkManifest, ManifestError> {
    require_non_empty(&raw.workloads, "workloads")?;
    require_non_empty(&raw.engines, "engines")?;
    require_non_empty(&raw.run.timeouts, "run.timeouts")?;
    require_non_empty(&raw.outputs.formats, "outputs.formats")?;

    let runs = NonZeroU64::new(raw.run.runs)
        .ok_or_else(|| ManifestError::new("run.runs", "must be greater than zero"))?;
    let policy = validate_run_policy(&raw.run.policy)?;
    let resources = validate_resources(raw.run.resources, "run.resources")?;
    let timeouts = validate_timeouts(raw.run.timeouts)?;
    let outputs = ManifestOutputs {
        directory: resolve_output_directory(base, &raw.outputs.directory)?,
        formats: validate_unique_values(raw.outputs.formats, "outputs.formats")?,
    };
    let workloads = validate_workloads(raw.workloads, base, &timeouts)?;
    let proof_required = workloads
        .iter()
        .flat_map(|workload| &workload.phases)
        .any(|phase| phase.is_proof_related());
    let engines = validate_engines(raw.engines, base, proof_required)?;

    Ok(BenchmarkManifest {
        manifest_path,
        manifest_version: raw.manifest_version,
        run: ManifestRun {
            warmups: raw.run.warmups,
            runs,
            policy,
            resources,
            timeouts,
        },
        outputs,
        workloads,
        engines,
    })
}

fn validate_run_policy(raw: &RawRunPolicy) -> Result<RunPolicy, ManifestError> {
    let parallel_attempts =
        NonZeroU64::new(raw.concurrency.parallel_attempts).ok_or_else(|| {
            ManifestError::new(
                "run.policy.concurrency.parallel_attempts",
                "must be greater than zero",
            )
        })?;
    let concurrency = Concurrency::new(raw.concurrency.mode, parallel_attempts).map_err(|_| {
        ManifestError::new(
            "run.policy.concurrency.parallel_attempts",
            "does not match concurrency mode",
        )
    })?;

    Ok(RunPolicy::new(RunPolicyParts {
        ordering_algorithm: non_empty_literal(raw.ordering_algorithm.as_str()),
        seed: raw.seed,
        concurrency,
        retry_policy: non_empty_literal(raw.retry_policy.as_str()),
        invalidation_policy: non_empty_literal(raw.invalidation_policy.as_str()),
        outlier_rule: non_empty_literal(raw.outlier_rule.as_str()),
        percentile_method: raw.percentile_method,
    }))
}

fn validate_resources(raw: RawResources, path: &str) -> Result<Resources, ManifestError> {
    require_non_empty(&raw.cpu_affinity, &format!("{path}.cpu_affinity"))?;
    let mut seen_cpu_indexes = HashSet::with_capacity(raw.cpu_affinity.len());
    for (index, cpu_index) in raw.cpu_affinity.iter().enumerate() {
        if !seen_cpu_indexes.insert(*cpu_index) {
            return Err(ManifestError::new(
                format!("{path}.cpu_affinity[{index}]"),
                "duplicate CPU affinity index",
            ));
        }
    }
    let worker_count = NonZeroU64::new(raw.worker_count).ok_or_else(|| {
        ManifestError::new(format!("{path}.worker_count"), "must be greater than zero")
    })?;
    validate_environment_variable_keys(&raw.environment_variables, path)?;
    validate_remote_profile(
        raw.execution_mode,
        raw.network_access,
        raw.remote.as_ref(),
        path,
    )?;

    Resources::new(ResourcesParts {
        power_profile: raw.power_profile,
        cpu_affinity: raw.cpu_affinity,
        cpu_limit: raw.cpu_limit,
        memory_limit_bytes: raw.memory_limit_bytes,
        worker_count,
        accelerator_allocation: raw.accelerator_allocation,
        environment_variables: raw.environment_variables,
        execution_mode: raw.execution_mode,
        network_access: raw.network_access,
        remote: raw.remote,
    })
    .map_err(|error| manifest_resource_error(path, &error))
}

fn validate_environment_variable_keys(
    variables: &BTreeMap<String, String>,
    path: &str,
) -> Result<(), ManifestError> {
    for key in variables.keys() {
        let field_path = format!("{path}.environment_variables.{key}");
        if !valid_environment_variable_key(key) {
            return Err(ManifestError::new(
                field_path,
                "environment variable names must be non-secret ASCII identifiers",
            ));
        }
        validate_non_secret_key(key, &field_path)?;
    }
    Ok(())
}

fn validate_remote_profile(
    execution_mode: ExecutionMode,
    network_access: bool,
    remote: Option<&RemoteProfile>,
    path: &str,
) -> Result<(), ManifestError> {
    match (execution_mode, network_access, remote.is_some()) {
        (ExecutionMode::Local, false, false) | (ExecutionMode::Remote, true, true) => Ok(()),
        (ExecutionMode::Local, true, _) => Err(ManifestError::new(
            format!("{path}.network_access"),
            "must be false for local execution",
        )),
        (ExecutionMode::Local, false, true) => Err(ManifestError::new(
            format!("{path}.remote"),
            "must be absent for local execution",
        )),
        (ExecutionMode::Remote, false, _) => Err(ManifestError::new(
            format!("{path}.network_access"),
            "must be true for remote execution",
        )),
        (ExecutionMode::Remote, true, false) => Err(ManifestError::new(
            format!("{path}.remote"),
            "is required for remote execution",
        )),
    }
}

fn manifest_resource_error(path: &str, error: &MetadataError) -> ManifestError {
    ManifestError::new(path, error.to_string())
}

fn validate_non_secret_key(key: &str, field_path: &str) -> Result<(), ManifestError> {
    if is_secret_like_key(key) {
        Err(ManifestError::new(
            field_path,
            "must not contain secret-like key names",
        ))
    } else {
        Ok(())
    }
}

fn valid_environment_variable_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn non_empty_literal(value: &'static str) -> NonEmptyString {
    NonEmptyString::new(value).expect("manifest policy literals must be non-empty")
}

fn validate_timeouts(raw: Vec<RawTimeout>) -> Result<Vec<PhaseTimeout>, ManifestError> {
    let mut seen = HashSet::with_capacity(raw.len());
    raw.into_iter()
        .enumerate()
        .map(|(index, timeout)| {
            let path = format!("run.timeouts[{index}]");
            if !seen.insert(timeout.phase) {
                return Err(ManifestError::new(
                    format!("{path}.phase"),
                    "duplicate timeout phase",
                ));
            }
            let limit_ms = NonZeroU64::new(timeout.limit_ms).ok_or_else(|| {
                ManifestError::new(format!("{path}.limit_ms"), "must be greater than zero")
            })?;
            if timeout.termination_grace_ms > timeout.limit_ms {
                return Err(ManifestError::new(
                    format!("{path}.termination_grace_ms"),
                    "must not exceed limit_ms",
                ));
            }
            Ok(PhaseTimeout {
                phase: timeout.phase,
                limit_ms,
                termination_grace_ms: timeout.termination_grace_ms,
            })
        })
        .collect()
}

fn validate_workloads(
    raw: Vec<RawWorkload>,
    base: &Path,
    timeouts: &[PhaseTimeout],
) -> Result<Vec<ManifestWorkload>, ManifestError> {
    let timeout_phases: HashSet<_> = timeouts.iter().map(|timeout| timeout.phase).collect();
    let mut workload_ids = HashSet::with_capacity(raw.len());

    raw.into_iter()
        .enumerate()
        .map(|(workload_index, workload)| {
            let path = format!("workloads[{workload_index}]");
            if !workload_ids.insert(workload.id.clone()) {
                return Err(ManifestError::new(
                    format!("{path}.id"),
                    "duplicate workload id",
                ));
            }
            require_non_empty(&workload.phases, &format!("{path}.phases"))?;
            require_non_empty(&workload.inputs, &format!("{path}.inputs"))?;
            let phases = validate_unique_values(workload.phases, &format!("{path}.phases"))?;
            for (phase_index, phase) in phases.iter().enumerate() {
                if !timeout_phases.contains(phase) {
                    return Err(ManifestError::new(
                        format!("{path}.phases[{phase_index}]"),
                        "selected phase has no timeout policy",
                    ));
                }
            }
            let security_target_bits =
                NonZeroU64::new(workload.security_target_bits).ok_or_else(|| {
                    ManifestError::new(
                        format!("{path}.security_target_bits"),
                        "must be greater than zero",
                    )
                })?;
            let specification = resolve_regular_file(
                base,
                &workload.specification,
                &format!("{path}.specification"),
            )?;
            let inputs = validate_inputs(workload.inputs, base, &path)?;

            Ok(ManifestWorkload {
                id: workload.id,
                revision: workload.revision,
                specification,
                implementation_lane: workload.implementation_lane,
                security_target_bits,
                phases,
                inputs,
            })
        })
        .collect()
}

fn validate_inputs(
    raw: Vec<RawInput>,
    base: &Path,
    workload_path: &str,
) -> Result<Vec<ManifestInput>, ManifestError> {
    let mut input_ids = HashSet::with_capacity(raw.len());
    raw.into_iter()
        .enumerate()
        .map(|(input_index, input)| {
            let path = format!("{workload_path}.inputs[{input_index}]");
            if !input_ids.insert(input.id.clone()) {
                return Err(ManifestError::new(
                    format!("{path}.id"),
                    "duplicate input id within workload",
                ));
            }
            let fixture = resolve_regular_file(base, &input.fixture, &format!("{path}.fixture"))?;
            let expected_output = resolve_regular_file(
                base,
                &input.expected_output,
                &format!("{path}.expected_output"),
            )?;
            Ok(ManifestInput {
                id: input.id,
                fixture,
                expected_output,
                visibility: input.visibility,
                preprocessing: input.preprocessing,
                commit_input: input.commit_input,
                commit_output: input.commit_output,
            })
        })
        .collect()
}

fn validate_engines(
    raw: Vec<RawEngine>,
    base: &Path,
    proof_required: bool,
) -> Result<Vec<ManifestEngine>, ManifestError> {
    let mut engine_ids = HashSet::with_capacity(raw.len());
    raw.into_iter()
        .enumerate()
        .map(|(engine_index, engine)| {
            let path = format!("engines[{engine_index}]");
            if !engine_ids.insert(engine.id.clone()) {
                return Err(ManifestError::new(
                    format!("{path}.id"),
                    "duplicate engine id",
                ));
            }
            if proof_required && engine.proof_modes.is_empty() {
                return Err(ManifestError::new(
                    format!("{path}.proof_modes"),
                    "must select at least one mode for proof-related phases",
                ));
            }
            let proof_modes =
                validate_unique_values(engine.proof_modes, &format!("{path}.proof_modes"))?;
            let adapter = resolve_regular_file(base, &engine.adapter, &format!("{path}.adapter"))?;
            validate_configuration_keys(&engine.configuration, &format!("{path}.configuration"))?;
            Ok(ManifestEngine {
                id: engine.id,
                adapter,
                proof_modes,
                configuration: engine.configuration,
            })
        })
        .collect()
}

fn validate_configuration_keys(
    values: &BTreeMap<String, Value>,
    path: &str,
) -> Result<(), ManifestError> {
    for (key, value) in values {
        let field_path = format!("{path}.{key}");
        validate_non_secret_key(key, &field_path)?;
        validate_json_configuration_keys(value, &field_path)?;
    }
    Ok(())
}

fn validate_json_configuration_keys(value: &Value, path: &str) -> Result<(), ManifestError> {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                let field_path = format!("{path}.{key}");
                validate_non_secret_key(key, &field_path)?;
                validate_json_configuration_keys(value, &field_path)?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_json_configuration_keys(value, &format!("{path}[{index}]"))?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn validate_unique_values<T>(values: Vec<T>, path: &str) -> Result<Vec<T>, ManifestError>
where
    T: Clone + Eq + std::hash::Hash,
{
    let mut seen = HashSet::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        if !seen.insert(value.clone()) {
            return Err(ManifestError::new(
                format!("{path}[{index}]"),
                "duplicate value",
            ));
        }
    }
    Ok(values)
}

fn require_non_empty<T>(values: &[T], path: &str) -> Result<(), ManifestError> {
    if values.is_empty() {
        Err(ManifestError::new(path, "must contain at least one item"))
    } else {
        Ok(())
    }
}

fn canonical_regular_file(path: &Path, field_path: &str) -> Result<PathBuf, ManifestError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        ManifestError::new(
            field_path,
            format!("could not resolve {}: {error}", path.display()),
        )
    })?;
    if !canonical.is_file() {
        return Err(ManifestError::new(
            field_path,
            format!("{} is not a regular file", canonical.display()),
        ));
    }
    Ok(canonical)
}

fn resolve_regular_file(
    base: &Path,
    path: &Path,
    field_path: &str,
) -> Result<ResolvedFile, ManifestError> {
    require_relative(path, field_path)?;
    canonical_regular_file(&base.join(path), field_path).map(ResolvedFile)
}

fn resolve_output_directory(base: &Path, path: &Path) -> Result<PathBuf, ManifestError> {
    const FIELD: &str = "outputs.directory";
    require_relative(path, FIELD)?;
    let resolved = normalize_path(&base.join(path));
    match fs::metadata(&resolved) {
        Ok(metadata) if metadata.is_dir() => fs::canonicalize(&resolved).map_err(|error| {
            ManifestError::new(
                FIELD,
                format!("could not resolve {}: {error}", resolved.display()),
            )
        }),
        Ok(_) => Err(ManifestError::new(
            FIELD,
            format!("{} is not a directory", resolved.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(resolved),
        Err(error) => Err(ManifestError::new(
            FIELD,
            format!("could not inspect {}: {error}", resolved.display()),
        )),
    }
}

fn require_relative(path: &Path, field_path: &str) -> Result<(), ManifestError> {
    if path.as_os_str().is_empty() {
        Err(ManifestError::new(field_path, "must not be empty"))
    } else if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        Err(ManifestError::new(
            field_path,
            "must be relative to the manifest",
        ))
    } else {
        Ok(())
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::require_relative;
    use super::{line_column, normalize_path, refine_deserialize_path};

    #[test]
    fn serde_diagnostic_paths_include_unknown_and_missing_fields() {
        assert_eq!(refine_deserialize_path("", "invalid type"), "manifest");
        assert_eq!(refine_deserialize_path(".", "invalid type"), "manifest");
        assert_eq!(
            refine_deserialize_path("workloads[1]", "unknown field `surprise`"),
            "workloads[1].surprise"
        );
        assert_eq!(
            refine_deserialize_path("workloads[1].inputs[0]", "missing field `fixture`"),
            "workloads[1].inputs[0].fixture"
        );
    }

    #[test]
    fn source_locations_and_lexical_paths_are_stable() {
        assert_eq!(line_column("first\nsecond", 8), (2, 3));
        assert_eq!(
            normalize_path(Path::new("suite/bench/../runs")),
            PathBuf::from("suite/runs")
        );
    }

    #[test]
    fn parent_paths_are_rejected() {
        for path in [
            Path::new("../fixture.bin"),
            Path::new("suite/../fixture.bin"),
        ] {
            let error = require_relative(path, "fixture")
                .expect_err("parent paths must not escape the manifest directory");
            assert_eq!(error.field_path(), "fixture");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_relative_and_rooted_paths_are_rejected() {
        for path in [Path::new(r"C:fixture.bin"), Path::new(r"\fixture.bin")] {
            let error = require_relative(path, "fixture")
                .expect_err("Windows-prefixed paths must not escape the manifest directory");
            assert_eq!(error.field_path(), "fixture");
        }
    }
}
