#[path = "mock_adapter/environment.rs"]
mod environment;
#[path = "mock_adapter/runner.rs"]
mod runner;
mod support;

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::{Fixture, assert_status, command};

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .arg(root.join("tools/create_mock_fixture.py"))
        .arg(fixture.0.join("mock benchmark"))
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(fixture.manifest().is_file());
    fixture
}

fn configure(fixture: &Fixture, fault: &str, target: &str) {
    let path = fixture.0.join("mock benchmark/mock.zkperf-adapter.json");
    let mut adapter: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    adapter["command"].as_array_mut().unwrap().extend([
        json!("--fault"),
        json!(fault),
        json!("--target"),
        json!(target),
    ]);
    fs::write(path, serde_json::to_vec(&adapter).unwrap()).unwrap();
}

fn run_path(fixture: &Fixture) -> PathBuf {
    fs::read_dir(fixture.0.join("mock benchmark/results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

fn operations(root: &Path) -> Vec<(Value, Value)> {
    let mut records = Vec::new();
    for attempt in fs::read_dir(root.join("logs")).unwrap() {
        let path = attempt.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        for operation in fs::read_dir(path).unwrap() {
            let path = operation.unwrap().path();
            let outcome =
                serde_json::from_slice(&fs::read(path.join("outcome.json")).unwrap()).unwrap();
            let request =
                serde_json::from_slice(&fs::read(path.join("request.json")).unwrap()).unwrap();
            records.push((request, outcome));
        }
    }
    records
}

#[test]
fn checked_in_fixture_runs_warmups_and_repetitions_without_an_sdk() {
    let fixture = fixture();
    let manifest = fixture.0.join("mock benchmark/zkperf.toml");
    let preview = command()
        .args(["run", "--dry-run", "--manifest"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert_status(&preview, 0);
    assert!(!fixture.0.join("mock benchmark/results").exists());
    let output = command()
        .args(["run", "--manifest"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Completed 3 jobs")
    );
    let root = run_path(&fixture);
    let record: Value = serde_json::from_slice(&fs::read(root.join("run.json")).unwrap()).unwrap();
    assert_eq!(record["state"], "completed");
    let records = operations(&root);
    assert_eq!(records.len(), 30);
    assert!(
        records
            .iter()
            .all(|(_, outcome)| outcome["outcome"] == "success")
    );
    let mut snapshots = Vec::new();
    for attempt in fs::read_dir(root.join("logs")).unwrap() {
        let mut artifacts = Vec::new();
        for operation in fs::read_dir(attempt.unwrap().path()).unwrap() {
            let response: Value = serde_json::from_slice(
                &fs::read(operation.unwrap().path().join("stdout.bin")).unwrap(),
            )
            .unwrap();
            for entry in response["artifacts"].as_array().unwrap() {
                artifacts.push((entry["id"].to_string(), entry["digest"].to_string()));
            }
        }
        artifacts.sort();
        assert_eq!(artifacts.len(), 8);
        snapshots.push(artifacts);
    }
    assert_eq!(snapshots.len(), 3);
    assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn mock_faults_retain_the_expected_cli_outcome_and_stop_at_the_selected_stage() {
    for (fault, expected) in [
        ("error", "adapter_error"),
        ("unsupported", "unsupported"),
        ("exit", "process_failed"),
        ("malformed", "protocol_error"),
        ("invalid-utf8", "protocol_error"),
        ("trailing", "protocol_error"),
        ("empty", "protocol_error"),
        ("schema", "protocol_error"),
        ("mismatch", "protocol_error"),
        ("stdout-overflow", "protocol_error"),
        ("artifact-escape", "artifact_error"),
        ("missing-artifact", "artifact_error"),
        ("bad-digest", "artifact_error"),
        ("bad-length", "artifact_error"),
    ] {
        let fixture = fixture();
        configure(&fixture, fault, "prove.initial");
        let output = command()
            .args(["run", "--warmups", "0", "--runs", "1", "--manifest"])
            .arg(fixture.0.join("mock benchmark/zkperf.toml"))
            .output()
            .unwrap();
        assert_status(&output, 6);
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("run evidence:")
        );
        let root = run_path(&fixture);
        let record: Value =
            serde_json::from_slice(&fs::read(root.join("run.json")).unwrap()).unwrap();
        assert_eq!(record["state"], "failed");
        environment::assert_capture(&root);
        let records = operations(&root);
        assert_eq!(records.len(), 8, "{fault}: {records:?}");
        for (request, outcome) in records {
            assert_eq!(
                outcome["outcome"],
                if request["operation"] == "prove" {
                    expected
                } else {
                    "success"
                },
                "{fault}: {outcome}"
            );
        }
    }
}

#[test]
fn graceful_and_forced_timeouts_keep_timeout_outcomes() {
    for fault in ["timeout", "ignore-cancel"] {
        let fixture = fixture();
        configure(&fixture, fault, "execute");
        let manifest = fixture.0.join("mock benchmark/zkperf.toml");
        let source = fs::read_to_string(&manifest).unwrap().replace(
            "phase = \"execution\"\nlimit_ms = 5_000",
            "phase = \"execution\"\nlimit_ms = 500",
        );
        fs::write(&manifest, source).unwrap();
        let output = command()
            .args(["run", "--warmups", "0", "--runs", "1", "--manifest"])
            .arg(manifest)
            .output()
            .unwrap();
        assert_status(&output, 6);
        environment::assert_capture(&run_path(&fixture));
        let (_, outcome) = operations(&run_path(&fixture))
            .into_iter()
            .find(|(request, _)| request["operation"] == "execute")
            .unwrap();
        assert_eq!(outcome["outcome"], "timed_out");
        assert_eq!(outcome["phase_duration_ns"], 500_000_000_u64);
        if fault == "timeout" {
            assert_eq!(outcome["exit_code"], 0, "{outcome}");
        } else {
            assert_ne!(outcome["exit_code"], 0, "{outcome}");
        }
    }
}

#[test]
fn stderr_flood_does_not_break_a_successful_benchmark() {
    let fixture = fixture();
    configure(&fixture, "stderr-flood", "execute");
    let output = command()
        .args(["run", "--warmups", "0", "--runs", "1", "--manifest"])
        .arg(fixture.0.join("mock benchmark/zkperf.toml"))
        .output()
        .unwrap();
    assert_status(&output, 0);
    let (_, outcome) = operations(&run_path(&fixture))
        .into_iter()
        .find(|(request, _)| request["operation"] == "execute")
        .unwrap();
    assert_eq!(outcome["outcome"], "success");
    assert_eq!(outcome["stderr_truncated"], true);
}
