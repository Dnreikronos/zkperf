#[path = "execution/regressions.rs"]
mod regressions;
mod support;

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::{Fixture, assert_status, command};

#[test]
fn run_executes_the_selected_command_and_preserves_failure_evidence() {
    let fixture = Fixture::new();
    assert!(fixture.manifest().is_file());
    let output = command()
        .args(["run", "--warmups", "0", "--runs", "1"])
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    assert_status(&output, 6);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("spawn"), "{stderr}");
    assert!(stderr.contains("run evidence:"), "{stderr}");
    let directory = fs::read_dir(fixture.0.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("run.json")).unwrap()).unwrap();
    assert_eq!(record["state"], "failed");
    assert!(record["environment"]["harness"]["version"].is_string());
    let logs = fs::read_dir(directory.join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    let operation = fs::read_dir(logs).unwrap().next().unwrap().unwrap().path();
    assert!(operation.join("outcome.json").is_file());
    assert!(operation.join("stderr.bin").is_file());
}

fn adapter(fixture: &Fixture, mode: &str) {
    let python = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let manifest = json!({"kind":"zkperf-adapter-manifest", "manifest_version":"1.0.0", "adapter_id":"mock", "display_name":"Test fixture",
        "protocol_versions":["1.0.0"], "command":[python.trim(), root.join("crates/zkperf-core/tests/fixtures/runner.py"),mode,root.join("examples/protocol-v1.json")]});
    fs::write(
        fixture.0.join("mock.zkperf-adapter.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn run_directory(fixture: &Fixture) -> PathBuf {
    fs::read_dir(fixture.0.join("results"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn run_completes_real_adapter_lifecycles_with_immutable_artifact_handoffs() {
    let fixture = Fixture::new();
    adapter(&fixture, "lifecycle");
    let output = command()
        .args(["run", "--warmups", "1", "--runs", "2"])
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Completed 3 jobs")
    );
    let root = run_directory(&fixture);
    let record: Value = serde_json::from_slice(&fs::read(root.join("run.json")).unwrap()).unwrap();
    assert_eq!(record["state"], "completed");
    let attempts: Vec<_> = fs::read_dir(root.join("logs")).unwrap().collect();
    assert_eq!(attempts.len(), 3);
    for attempt in attempts {
        let mut operations = Vec::new();
        for operation in fs::read_dir(attempt.unwrap().path()).unwrap() {
            let record: Value = serde_json::from_slice(
                &fs::read(operation.unwrap().path().join("outcome.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(record["outcome"], "success", "{record}");
            operations.push(record["operation"].as_str().unwrap().to_owned());
        }
        operations.sort();
        assert_eq!(
            operations,
            [
                "capabilities",
                "execute",
                "metadata",
                "prepare",
                "prepare",
                "prepare",
                "prove",
                "prove",
                "verify"
            ]
        );
    }
}

#[test]
fn invalid_output_artifacts_fail_the_run_and_keep_diagnostics() {
    for mode in ["bad-artifact", "oversized-artifact"] {
        let fixture = Fixture::new();
        adapter(&fixture, mode);
        let output = command()
            .args(["run", "--warmups", "0", "--runs", "1"])
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert_status(&output, 6);
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("ArtifactError")
        );
        let record: Value =
            serde_json::from_slice(&fs::read(run_directory(&fixture).join("run.json")).unwrap())
                .unwrap();
        assert_eq!(record["state"], "failed");
    }
}

#[cfg(unix)]
#[test]
fn ctrl_c_cancels_the_cli_and_terminates_adapter_descendants() {
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    adapter(&fixture, "cancel-cli");
    let pid = fixture.0.join("pid");
    let survivor = fixture.0.join("survived");
    let source = fs::read_to_string(fixture.manifest()).unwrap().replace(
        "environment_variables = {}",
        &format!(
            "environment_variables = {{ PID_FILE = {}, SURVIVOR_FILE = {} }}",
            json!(pid),
            json!(survivor)
        ),
    );
    fs::write(fixture.manifest(), source).unwrap();
    let mut child = command()
        .args(["run", "--warmups", "0", "--runs", "1"])
        .current_dir(&fixture.0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !pid.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "adapter did not start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI cancellation hung");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert_status(&output, 130);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("error[cancelled]")
    );
    let record: Value =
        serde_json::from_slice(&fs::read(run_directory(&fixture).join("run.json")).unwrap())
            .unwrap();
    assert_eq!(record["state"], "failed");
    assert!(record["environment"]["host"].is_object());
    let retained = zkperf_core::RunRecord::load(run_directory(&fixture)).unwrap();
    assert_eq!(
        retained.environment_digest().value(),
        Some(&retained.environment().value().unwrap().digest())
    );
    std::thread::sleep(Duration::from_millis(2100));
    assert!(!survivor.exists());
}
