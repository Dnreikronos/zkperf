mod support;

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use support::{Fixture, assert_status, command, fixture_root};

fn config(output: &Output) -> Value {
    assert_status(output, 0);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn manifest_values_are_retained_without_overrides() {
    let output = command()
        .args(["validate", "--print-config"])
        .output()
        .unwrap();
    let value = config(&output);
    assert_eq!(value["run"]["warmups"], 1);
    assert_eq!(value["run"]["runs"], 3);
    assert_eq!(value["outputs"]["formats"], json!(["json", "terminal"]));
    assert_eq!(
        Path::new(value["outputs"]["directory"].as_str().unwrap()),
        fixture_root().join("results")
    );
}

#[test]
fn environment_overrides_manifest_and_flags_override_environment() {
    let mut cmd = command();
    cmd.args(["run", "--print-config"])
        .env("ZKPERF_WARMUPS", "2")
        .env("ZKPERF_RUNS", "7")
        .env("ZKPERF_OUTPUT_DIR", "environment-results")
        .env("ZKPERF_FORMAT", "html,csv");
    let value = config(&cmd.output().unwrap());
    assert_eq!(value["run"]["warmups"], 2);
    assert_eq!(value["run"]["runs"], 7);
    assert_eq!(value["outputs"]["formats"], json!(["html", "csv"]));
    assert_eq!(
        Path::new(value["outputs"]["directory"].as_str().unwrap()),
        fixture_root().join("environment-results")
    );
    cmd.args([
        "--warmups",
        "0",
        "--runs",
        "9",
        "--format",
        "json",
        "--output-dir",
        "flag-results",
    ]);
    let value = config(&cmd.output().unwrap());
    assert_eq!(value["run"]["warmups"], 0);
    assert_eq!(value["run"]["runs"], 9);
    assert_eq!(value["outputs"]["formats"], json!(["json"]));
    assert_eq!(
        Path::new(value["outputs"]["directory"].as_str().unwrap()),
        fixture_root().join("flag-results")
    );
}

#[test]
fn malformed_environment_is_masked_only_for_explicitly_overridden_fields() {
    for (key, flag, value) in [
        ("ZKPERF_WARMUPS", "--warmups", "0"),
        ("ZKPERF_RUNS", "--runs", "4"),
        ("ZKPERF_FORMAT", "--format", "json"),
        ("ZKPERF_LOG", "--log-level", "off"),
    ] {
        let output = command()
            .args(["validate", "--print-config", flag, value])
            .env(key, "private-invalid-value")
            .output()
            .unwrap();
        config(&output);
        let output = command()
            .args(["validate", "--print-config"])
            .env(key, "private-invalid-value")
            .output()
            .unwrap();
        assert_status(&output, 3);
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(key));
        assert!(!stderr.contains("private-invalid-value"));
    }
    for key in ["ZKPERF_MANIFEST", "ZKPERF_OUTPUT_DIR"] {
        let output = command().arg("validate").env(key, "").output().unwrap();
        assert_status(&output, 3);
    }
    let output = command()
        .args(["validate", "--output-dir", "results"])
        .env("ZKPERF_OUTPUT_DIR", "")
        .output()
        .unwrap();
    assert_status(&output, 0);
}

#[test]
fn manifest_selection_and_relative_paths_are_independent_of_working_directory() {
    let fixture = Fixture::new();
    let source = fs::read(fixture.manifest()).unwrap();
    let output = command()
        .args(["validate", "--print-config"])
        .env("ZKPERF_MANIFEST", fixture.manifest())
        .output()
        .unwrap();
    let value = config(&output);
    let root = fixture.0.canonicalize().unwrap();
    assert_eq!(
        Path::new(value["manifest_path"].as_str().unwrap()),
        root.join("zkperf.toml")
    );
    assert_eq!(
        Path::new(value["outputs"]["directory"].as_str().unwrap()),
        root.join("results")
    );
    let output = command()
        .args(["validate", "--print-config", "--manifest"])
        .arg(fixture.manifest())
        .args(["--output-dir", "override-results"])
        .env("ZKPERF_MANIFEST", "missing.toml")
        .output()
        .unwrap();
    let value = config(&output);
    assert_eq!(
        Path::new(value["outputs"]["directory"].as_str().unwrap()),
        root.join("override-results")
    );
    assert!(!fixture.0.join("override-results").exists());
    assert_eq!(fs::read(fixture.manifest()).unwrap(), source);
}

#[test]
fn invalid_effective_settings_are_configuration_errors() {
    for args in [
        vec!["validate", "--format", "json,json"],
        vec!["validate", "--output-dir", "../escape"],
        vec!["validate", "--output-dir", "workload.md"],
        vec!["validate", "--manifest", "missing.toml"],
    ] {
        let output = command().args(args).output().unwrap();
        assert_status(&output, 3);
        assert!(output.stdout.is_empty());
    }
    for (key, value) in [
        ("ZKPERF_RUNS", "0"),
        ("ZKPERF_FORMAT", "json,json"),
        ("ZKPERF_FORMAT", ""),
        ("ZKPERF_WARMUPS", "-1"),
    ] {
        let output = command().arg("validate").env(key, value).output().unwrap();
        assert_status(&output, 3);
    }
}

#[test]
fn configuration_output_is_redacted_deterministic_and_uses_effective_values() {
    let fixture = Fixture::new();
    let source = fs::read_to_string(fixture.manifest()).unwrap().replace(
        "environment_variables = {}",
        "environment_variables = { BENCH_MODE = 'private-setting' }",
    );
    fs::write(fixture.manifest(), source.as_bytes()).unwrap();
    let mut cmd = command();
    cmd.current_dir(&fixture.0)
        .args(["validate", "--print-config", "--runs", "12"]);
    let first = cmd.output().unwrap();
    let second = cmd.output().unwrap();
    let value = config(&first);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(value["run"]["runs"], 12);
    assert_eq!(
        value["run"]["resources"]["environment_variables"]["BENCH_MODE"],
        "[redacted]"
    );
    assert!(!String::from_utf8_lossy(&first.stdout).contains("private-setting"));
    assert!(!String::from_utf8_lossy(&first.stderr).contains("private-setting"));
    assert_eq!(fs::read_to_string(fixture.manifest()).unwrap(), source);
}

#[test]
fn overrides_do_not_repair_invalid_manifests() {
    let fixture = Fixture::new();
    let source = fs::read_to_string(fixture.manifest())
        .unwrap()
        .replace("runs = 3", "runs = 0");
    fs::write(fixture.manifest(), source).unwrap();
    let output = command()
        .current_dir(&fixture.0)
        .args(["validate", "--runs", "10"])
        .env("ZKPERF_RUNS", "5")
        .output()
        .unwrap();
    assert_status(&output, 3);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("run.runs")
    );
}
