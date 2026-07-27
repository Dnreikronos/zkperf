use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::num::NonZeroU64;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::{
    BenchmarkManifest, ManifestEngine, ManifestError, ManifestInput, ManifestOutputs,
    ManifestPhase, ManifestRun, ManifestWorkload, OutputFormat, PhaseTimeout, ResolvedFile,
};
use crate::{Commitment, ImplementationLane, InputVisibility, NonEmptyString, SchemaVersion, Slug};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    manifest_version: SchemaVersion,
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
    timeouts: Vec<RawTimeout>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTimeout {
    phase: ManifestPhase,
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
    phases: Vec<ManifestPhase>,
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
            timeouts,
        },
        outputs,
        workloads,
        engines,
    })
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
            Ok(ManifestEngine {
                id: engine.id,
                adapter,
                proof_modes,
                configuration: engine.configuration,
            })
        })
        .collect()
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
    } else if path.is_absolute() {
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

    use super::{line_column, normalize_path, refine_deserialize_path};

    #[test]
    fn serde_diagnostic_paths_include_unknown_and_missing_fields() {
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
}
