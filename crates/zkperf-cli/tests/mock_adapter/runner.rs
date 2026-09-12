use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};
use zkperf_core::{
    AdapterInvocation, BenchmarkManifest, BenchmarkPlan, CancellationToken, OperationOutcome,
    OperationResult, RunDirectory, RunnerLimits, run_operation,
};

use super::fixture;
use super::support::Fixture;

fn invocation(fixture: &Fixture, fault: &str) -> AdapterInvocation {
    let adapter: Value = serde_json::from_slice(
        &fs::read(fixture.0.join("mock benchmark/mock.zkperf-adapter.json")).unwrap(),
    )
    .unwrap();
    let command = adapter["command"].as_array().unwrap();
    let examples: Value =
        serde_json::from_str(include_str!("../../../../examples/protocol-v1.json")).unwrap();
    let mut arguments: Vec<_> = command[1..]
        .iter()
        .map(|value| value.as_str().unwrap().into())
        .collect();
    arguments.extend(["--fault".into(), fault.into()]);
    AdapterInvocation {
        executable: command[0].as_str().unwrap().into(),
        arguments,
        environment: BTreeMap::new(),
        request: examples["exchanges"][0]["request"].clone(),
        limits: RunnerLimits::default(),
        graceful_cancellation: true,
    }
}

fn run(
    fixture: &Fixture,
    invocation: &AdapterInvocation,
    token: &CancellationToken,
) -> OperationResult {
    let manifest = BenchmarkManifest::load(fixture.0.join("mock benchmark/zkperf.toml")).unwrap();
    let plan = BenchmarkPlan::build(manifest).unwrap();
    let mut run = RunDirectory::create(&plan).unwrap();
    let workspace = run.attempt(&plan.jobs()[0], 0).unwrap();
    run_operation(&mut run, &workspace, invocation, token).unwrap()
}

#[test]
fn user_cancellation_remains_distinct_from_mock_timeouts() {
    let fixture = fixture();
    let invocation = invocation(&fixture, "timeout");
    let token = CancellationToken::default();
    let trigger = token.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        trigger.cancel();
    });
    let result = run(&fixture, &invocation, &token);
    thread.join().unwrap();
    assert_eq!(result.record.outcome, OperationOutcome::Cancelled);
}

#[test]
fn blocked_input_fault_is_supervised_before_request_decoding() {
    let fixture = fixture();
    let mut invocation = invocation(&fixture, "blocked-input");
    invocation.request["timeout"]["limit_ns"] = json!(300_000_000);
    invocation.request["timeout"]["termination_grace_ns"] = json!(10_000_000);
    invocation.request["operation"] = json!("metadata");
    invocation.request["params"] =
        json!({"configuration": {"padding": "X".repeat(1_048_576)}, "artifacts": []});
    let result = run(&fixture, &invocation, &CancellationToken::default());
    assert_eq!(result.record.outcome, OperationOutcome::TimedOut);
    assert_eq!(result.record.phase_duration_ns, 300_000_000);
    assert!(result.response.is_none());
}

#[cfg(unix)]
#[test]
fn mock_signal_exit_is_a_process_failure() {
    let fixture = fixture();
    let result = run(
        &fixture,
        &invocation(&fixture, "signal"),
        &CancellationToken::default(),
    );
    assert_eq!(result.record.outcome, OperationOutcome::ProcessFailed);
    assert_eq!(result.record.signal, Some(15));
}
