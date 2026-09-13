use serde_json::{Value, json};

use crate::{
    ArtifactKind, ArtifactRequest, BenchmarkPlan, NonEmptyString, Observed, PlannedJob,
    RunDirectory, RunError, Sha256Digest,
};

pub(super) fn initial(run: &mut RunDirectory, plan: &BenchmarkPlan) -> Result<(), RunError> {
    for job in plan.jobs() {
        save(run, job, "initial", &Value::Null, &Value::Null, &[])?;
    }
    Ok(())
}

pub(super) fn save(
    run: &mut RunDirectory,
    job: &PlannedJob,
    stage: &str,
    runtime: &Value,
    capabilities: &Value,
    prepared: &[Value],
) -> Result<(), RunError> {
    let pending = gap(
        "not_observed",
        "This metadata has not been returned by a successful adapter operation.",
    );
    let mut identity = serde_json::Map::new();
    for key in [
        "adapter",
        "engine",
        "sdk",
        "toolchain",
        "proof_system",
        "backend",
        "verifier",
    ] {
        identity.insert(
            key.into(),
            runtime.get(key).cloned().unwrap_or_else(|| pending.clone()),
        );
    }
    if runtime.is_null() {
        if let Some(adapter) = capabilities.get("adapter") {
            identity.insert("adapter".into(), adapter.clone());
        }
    }
    let proof_mode = job.proof_mode().map_or_else(
        || gap("not_applicable", "The job does not select a proof mode."),
        |mode| {
            capabilities["proof_modes"]
                .as_array()
                .and_then(|modes| {
                    modes
                        .iter()
                        .find(|entry| entry["id"] == mode.as_str())
                        .cloned()
                })
                .unwrap_or_else(|| pending.clone())
        },
    );
    let mut guests: Vec<_> = prepared.iter().filter(|entry| entry["kind"] == "guest_program").map(|entry| {
        json!({"digest":entry["digest"], "byte_length":entry["byte_length"], "media_type":entry["media_type"]})
    }).collect();
    guests.sort_by_key(Value::to_string);
    let guest = &runtime["extensions"]["org.zkperf.guest"];
    let data = json!({
        "engine_id":job.engine_id(),
        "requested_proof_mode":job.proof_mode().map_or_else(|| gap("not_applicable", "The job does not select a proof mode."), |mode| json!(mode)),
        "identity":identity,
        "resolved_configuration":runtime.get("configuration").map_or_else(|| pending.clone(), safe_configuration),
        "proof_mode":proof_mode,
        "guest":{
            "source_revision":guest.get("source_revision").filter(|value| value.as_str().is_some_and(|text| !text.trim().is_empty())).cloned().unwrap_or_else(|| pending.clone()),
            "compiler":guest_compiler(&guest["compiler"]),
            "artifacts":if guests.is_empty() { pending } else { json!(guests) }
        }
    });
    let digest = Sha256Digest::from_bytes(crate::digest::hash_bytes(&serde_json::to_vec(&data)?));
    run.store(&ArtifactRequest {
        path: format!("metadata/{:010}/{stage}.json", job.position()),
        name: NonEmptyString::new("execution configuration").unwrap(),
        kind: ArtifactKind::Other,
        media_type: "application/json".into(),
        attempt_id: None,
    }, &serde_json::to_vec_pretty(&json!({
        "capture_version":"1.0.0", "job_id":job.id(), "stage":stage, "data":data, "digest":digest
    }))?)?;
    Ok(())
}

fn gap(code: &str, message: &str) -> Value {
    serde_json::to_value(Observed::<String>::gap(code, message)).unwrap()
}

fn safe_configuration(value: &Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if crate::manifest::is_secret_like_key(key) || key == "flags" {
                        Value::String("[redacted]".into())
                    } else {
                        safe_configuration(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(safe_configuration).collect()),
        _ => value.clone(),
    }
}

fn guest_compiler(value: &Value) -> Value {
    let field = |key: &str| value[key].as_str().filter(|text| !text.trim().is_empty());
    match (field("name"), field("version")) {
        (Some(name), Some(version)) => json!({
            "name":name,
            "version":version,
            "flags":gap("excluded", "Arbitrary compiler flags may contain private paths or secrets.")
        }),
        _ => gap(
            "not_observed",
            "Guest compiler name and version were not supplied.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{guest_compiler, safe_configuration};
    use serde_json::json;

    #[test]
    fn malformed_guest_metadata_is_explicit_and_private_flags_are_excluded() {
        for value in [
            json!(null),
            json!({}),
            json!({"name":"rustc"}),
            json!({"name":"rustc", "version":" "}),
        ] {
            assert_eq!(guest_compiler(&value)["availability"], "unavailable");
        }
        let value =
            guest_compiler(&json!({"name":"rustc", "version":"1.85", "flags":["private-path"]}));
        assert_eq!(value["version"], "1.85");
        assert_eq!(value["flags"]["availability"], "unavailable");
        assert!(!value.to_string().contains("private-path"));
        let value =
            safe_configuration(&json!({"nested":[{"apiKey":"private-key", "backend":"cpu"}]}));
        assert!(!value.to_string().contains("private-key"));
        assert_eq!(value["nested"][0]["backend"], "cpu");
    }
}
