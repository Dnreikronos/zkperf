use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

use crate::RunError;

pub(crate) fn invalid(message: &str) -> RunError {
    RunError::Serialization(<serde_json::Error as serde::de::Error>::custom(message))
}

pub(super) fn request_id(id: &str) -> Result<(), RunError> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|error| invalid(&error.to_string()))?;
    if parsed.to_string() != id {
        return Err(invalid("request_id must be a canonical UUID"));
    }
    Ok(())
}

pub(crate) struct Validator(Arc<jsonschema::Validator>);

impl Validator {
    pub(crate) fn new() -> Result<Self, RunError> {
        static VALIDATOR: OnceLock<Result<Arc<jsonschema::Validator>, String>> = OnceLock::new();
        VALIDATOR
            .get_or_init(|| {
                let schema: Value = serde_json::from_str(include_str!(
                    "../../../../schemas/adapter-protocol-v1.schema.json"
                ))
                .map_err(|error| error.to_string())?;
                jsonschema::options()
                    .should_validate_formats(true)
                    .build(&schema)
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map(|validator| Self(Arc::clone(validator)))
            .map_err(|error| invalid(error))
    }

    pub(crate) fn validate(&self, value: &Value) -> Result<(), RunError> {
        self.0.validate(value).map_err(|error| {
            invalid(&format!(
                "protocol schema violation at {} ({})",
                error.instance_path, error.schema_path
            ))
        })
    }

    pub(super) fn request(&self, request: &Value) -> Result<(), RunError> {
        self.validate(request)?;
        if request.get("params").is_none() || request["protocol_version"] != "1.0.0" {
            return Err(invalid("expected a v1 request"));
        }
        if request["workspace"]
            != json!({"inputs_dir":"inputs","outputs_dir":"outputs","control_dir":"control"})
        {
            return Err(invalid(
                "runner workspace roots must be inputs, outputs, control",
            ));
        }
        request_id(request["request_id"].as_str().unwrap())
    }

    pub(super) fn response(&self, request: &Value, bytes: &[u8]) -> Result<Value, String> {
        let response: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        self.validate(&response)
            .map_err(|error| error.to_string())?;
        if response.get("status").is_none() {
            return Err("expected a response envelope".into());
        }
        for field in ["protocol", "protocol_version", "request_id", "operation"] {
            if request[field] != response[field] {
                return Err(format!("response {field} does not match request"));
            }
        }
        let artifacts = response["artifacts"].as_array().unwrap();
        let mut ids = HashSet::new();
        for artifact in artifacts {
            if !ids.insert(artifact["id"].as_str().unwrap()) {
                return Err("duplicate response artifact ID".into());
            }
            if !artifact["path"].as_str().unwrap().starts_with("outputs/") {
                return Err("response artifact must belong to outputs".into());
            }
        }
        let operation = request["operation"].as_str().unwrap();
        let params = &request["params"];
        if response["status"] != "success" {
            let expected = match operation {
                "prepare" => match params["stage"].as_str().unwrap() {
                    "environment" => "preparation",
                    stage => stage,
                },
                "execute" => "execution",
                "prove" if params["stage"] == "transform" => "compression",
                "prove" => "proving",
                "verify" => "verification",
                other => other,
            };
            let phase = response["error"]["phase"].as_str().unwrap();
            if ![expected, "protocol", "artifact", "cleanup"].contains(&phase) {
                return Err("error phase does not match operation".into());
            }
            return Ok(response);
        }
        let result = &response["result"];
        for (key, value) in result.as_object().unwrap() {
            let references = if key.ends_with("_artifact_ids") {
                value.as_array().unwrap().iter().collect::<Vec<_>>()
            } else if key.ends_with("_artifact_id") {
                vec![value]
            } else {
                Vec::new()
            };
            for id in references {
                let artifact = artifacts
                    .iter()
                    .find(|artifact| artifact["id"] == *id)
                    .ok_or_else(|| format!("unknown result artifact {id}"))?;
                let kind = match key.as_str() {
                    "canonical_output_artifact_id" => Some("canonical_output"),
                    "execution_artifact_id" => Some("execution_trace"),
                    "proof_artifact_id" => Some("proof"),
                    "public_values_artifact_id" => Some("public_values"),
                    _ => None,
                };
                if kind.is_some_and(|kind| artifact["kind"] != kind) {
                    return Err(format!("wrong artifact kind for {key}"));
                }
                if key == "canonical_output_artifact_id"
                    && artifact["digest"] != params["benchmark"]["expected_output_digest"]
                {
                    return Err("canonical output digest does not match benchmark".into());
                }
            }
        }
        for field in ["stage", "proof_mode_id", "transformation_id"] {
            if (operation == "prove" || (operation == "prepare" && field == "stage"))
                && params[field] != result[field]
            {
                return Err(format!("result {field} does not match request"));
            }
        }
        if operation == "verify"
            && (result["output_digest"] != params["expected_output_digest"]
                || result["commitment_digests"] != params["expected_commitment_digests"]
                || params["expected_output_digest"]
                    != params["benchmark"]["expected_output_digest"])
        {
            return Err("verification statement does not match request".into());
        }
        Ok(response)
    }
}
