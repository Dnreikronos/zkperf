use std::collections::HashSet;

use serde_json::{Value, json};

use crate::{RunError, runner::wire::invalid};

pub(super) fn validate(capabilities: &Value, adapter_id: &str) -> Result<(), RunError> {
    if capabilities["adapter"]["id"] != adapter_id
        || !capabilities["supported_protocol_versions"]
            .as_array()
            .unwrap()
            .contains(&json!("1.0.0"))
    {
        return Err(invalid(
            "capability_mismatch: manifest/runtime identity or protocol",
        ));
    }
    let mut expected = HashSet::new();
    for (operation, capability) in capabilities["operations"].as_object().unwrap() {
        if capability["supported"] != true {
            continue;
        }
        if let Some(stages) = capability["stages"].as_array() {
            for stage in stages {
                if stage != "environment" {
                    expected.insert((operation.as_str(), stage.as_str().unwrap()));
                }
            }
        } else if ["execute", "verify"].contains(&operation.as_str()) {
            expected.insert((operation.as_str(), ""));
        }
    }
    for boundary in capabilities["measurement_boundaries"].as_array().unwrap() {
        let operation = boundary["operation"].as_str().unwrap();
        let stage = boundary["stage"].as_str().unwrap_or("");
        if !expected.remove(&(operation, stage)) {
            return Err(invalid("duplicate or unadvertised measurement boundary"));
        }
        let name = match (operation, stage) {
            ("prepare", "build") => "build",
            ("prepare", "setup") => "setup",
            ("execute", "") => "execution",
            ("prove", "initial") => "proving",
            ("prove", "transform") => "compression",
            ("verify", "") => "verification",
            _ => return Err(invalid("invalid boundary")),
        };
        if boundary["phase"]["kind"] == "standard" {
            if boundary["phase"]["name"] != name {
                return Err(invalid("incorrect operation phase boundary"));
            }
        } else {
            let valid = (name == "build"
                && boundary["phase"]["components"] == json!(["build", "setup"]))
                || (name == "proving"
                    && boundary["phase"]["components"] == json!(["execution", "proving"]));
            if !valid {
                return Err(invalid("incorrect combined boundary"));
            }
        }
    }
    if !expected.is_empty() {
        return Err(invalid("missing supported measurement boundary"));
    }
    Ok(())
}

pub(super) fn input_artifacts(params: &Value) -> Vec<&Value> {
    let mut entries = Vec::new();
    for key in ["artifacts", "input_artifacts", "prepared_artifacts"] {
        if let Some(values) = params[key].as_array() {
            entries.extend(values);
        }
    }
    for key in ["canonical_input", "proof"] {
        if let Some(value) = params.get(key) {
            entries.push(value);
        }
    }
    entries
}

pub(super) fn require_phases(
    capabilities: &Value,
    phases: &[crate::StandardPhase],
) -> Result<(), RunError> {
    let boundaries = capabilities["measurement_boundaries"].as_array().unwrap();
    for phase in phases {
        let value = serde_json::to_value(phase)?;
        if !boundaries.iter().any(|boundary| {
            boundary["phase"]["kind"] == "standard" && boundary["phase"]["name"] == value
        }) {
            return Err(invalid(&format!(
                "selected phase {value} has no supported separate boundary"
            )));
        }
    }
    Ok(())
}
