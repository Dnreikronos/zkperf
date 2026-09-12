use serde_json::{Value, json};

use crate::{RunError, StandardPhase, runner::wire::invalid};

use super::session::{InvocationResult, Session};

pub(super) fn execute(session: &mut Session<'_>) -> Result<(), RunError> {
    session.negotiate()?;
    let phases = session.workload.phases();
    if phases.contains(&StandardPhase::EndToEnd) {
        return Err(invalid(
            "end_to_end aggregation is not available; select explicit phases",
        ));
    }
    super::capabilities::require_phases(&session.capabilities, phases)?;
    let mode = session
        .job
        .proof_mode()
        .map(|mode| mode.as_str().to_owned());
    let proof_mode = if let Some(mode) = &mode {
        Some(
            session.capabilities["proof_modes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == *mode)
                .ok_or_else(|| invalid("selected proof mode is not advertised"))?
                .clone(),
        )
    } else {
        None
    };
    for boundary in session.capabilities["measurement_boundaries"]
        .as_array()
        .unwrap()
    {
        if boundary["phase"]["kind"] == "combined" {
            return Err(invalid(
                "combined phase orchestration is not available; component timings cannot be inferred",
            ));
        }
    }
    let configuration = json!(session.engine.configuration());
    let benchmark = session.benchmark.clone();
    let needs_execution = phases.iter().any(|phase| {
        matches!(
            phase,
            StandardPhase::Execution
                | StandardPhase::Proving
                | StandardPhase::Compression
                | StandardPhase::Verification
        )
    });
    let advertised = session.capabilities["operations"]["prepare"]["stages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let stages = ["environment", "build", "setup"]
        .into_iter()
        .filter(|stage| advertised.contains(&json!(stage)));
    let mut prepared = Vec::<Value>::new();
    for stage in stages {
        let name = stage;
        let mut params = json!({"stage":stage,"benchmark":benchmark,"configuration":configuration,"input_artifacts":prepared});
        if name != "environment" {
            params["cache"] = json!({"state":"cold"});
        }
        let result = session.invoke("prepare", params, name)?;
        if let Some(ids) = result.result["prepared_artifact_ids"].as_array() {
            for id in ids {
                prepared.push(result.artifact(id)?);
            }
        }
    }
    if !needs_execution {
        return Ok(());
    }
    if session.capabilities["operations"]["execute"]["supported"] != true {
        return Err(invalid("execution is unsupported"));
    }
    let mut params = json!({"benchmark":benchmark,"configuration":configuration,
        "canonical_input":session.canonical_input,"prepared_artifacts":prepared});
    if let Some(mode) = &mode {
        params["proof_mode_id"] = mode.clone().into();
    }
    let execution = session.invoke("execute", params, "execution")?;
    if !phases.iter().any(|phase| {
        matches!(
            phase,
            StandardPhase::Proving | StandardPhase::Compression | StandardPhase::Verification
        )
    }) {
        return Ok(());
    }
    prove_and_verify(
        session,
        &execution,
        &prepared,
        proof_mode
            .as_ref()
            .ok_or_else(|| invalid("proving requires a proof mode"))?,
    )
}

fn prove_and_verify(
    session: &mut Session<'_>,
    execution: &InvocationResult,
    prepared: &[Value],
    mode: &Value,
) -> Result<(), RunError> {
    if session.capabilities["operations"]["prove"]["supported"] != true {
        return Err(invalid("proving is unsupported"));
    }
    let benchmark = session.benchmark.clone();
    let configuration = json!(session.engine.configuration());
    let mut inputs = prepared.to_vec();
    inputs.push(execution.artifact(&execution.result["execution_artifact_id"])?);
    let initial = session.invoke(
        "prove",
        json!({"stage":"initial","benchmark":benchmark,
        "configuration":configuration,"proof_mode_id":mode["id"],"input_artifacts":inputs}),
        "proving",
    )?;
    let mut proof = initial.artifact(&initial.result["proof_artifact_id"])?;
    for transformation in mode["transformations"].as_array().unwrap() {
        let mut inputs = prepared.to_vec();
        inputs.push(proof);
        let transformed = session.invoke("prove", json!({"stage":"transform","transformation_id":transformation["id"],
            "benchmark":benchmark,"configuration":configuration,"proof_mode_id":mode["id"],"input_artifacts":inputs}), "compression")?;
        proof = transformed.artifact(&transformed.result["proof_artifact_id"])?;
    }
    if session
        .workload
        .phases()
        .contains(&StandardPhase::Verification)
    {
        if session.capabilities["operations"]["verify"]["supported"] != true {
            return Err(invalid("verification is unsupported"));
        }
        let commitments = execution.result["commitment_digests"].clone();
        let verification = session.invoke("verify", json!({"benchmark":benchmark,"configuration":configuration,
            "proof_mode_id":mode["id"],"proof":proof,
            "statement":{"workload":benchmark["case_id"],"input_commitment":commitments["input"]["value"],"output_commitment":commitments["output"]["value"]},
            "expected_output_digest":benchmark["expected_output_digest"],"expected_commitment_digests":commitments}), "verification")?;
        if verification.result["verdict"] != "accepted" {
            return Err(invalid("proof verification rejected the statement"));
        }
    }
    Ok(())
}
