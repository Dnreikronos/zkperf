mod support;

use std::fs;

use serde_json::Value;
use support::{Fixture, assert_status, command};

#[test]
fn dry_run_prints_deterministic_json_with_effective_configuration_and_provenance() {
    let fixture = Fixture::new();
    let source = fs::read_to_string(fixture.manifest()).unwrap().replace(
        "environment_variables = {}",
        "environment_variables = { BENCH_MODE = 'private-setting' }",
    );
    fs::write(fixture.manifest(), &source).unwrap();
    let mut cmd = command();
    cmd.args(["run", "--dry-run", "--warmups", "0", "--runs", "2"])
        .arg("--manifest")
        .arg(fixture.manifest())
        .env("ZKPERF_WARMUPS", "3")
        .env("ZKPERF_RUNS", "5");
    let first = cmd.output().unwrap();
    let second = cmd.output().unwrap();
    assert_status(&first, 0);
    assert_status(&second, 0);
    assert_eq!(first.stdout, second.stdout);
    let plan: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(plan["plan_version"], "1.0.0");
    assert_eq!(plan["manifest"]["run"]["warmups"], 0);
    assert_eq!(plan["manifest"]["run"]["runs"], 2);
    assert_eq!(plan["files"].as_array().unwrap().len(), 4);
    let jobs = plan["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    for (position, job) in jobs.iter().enumerate() {
        assert_eq!(
            job["id"],
            format!("{}:{position}", plan["id"].as_str().unwrap())
        );
        assert_eq!(job["warmup"], false);
        assert_eq!(job["repetition"], position);
        assert_eq!(job["engine_id"], "mock");
        assert_eq!(job["workload_id"], "sha256");
        assert_eq!(job["input_id"], "small");
        assert_eq!(job["proof_mode"], "default");
    }
    assert!(String::from_utf8_lossy(&first.stdout).contains("[redacted]"));
    assert!(!String::from_utf8_lossy(&first.stdout).contains("private-setting"));
    assert!(!String::from_utf8_lossy(&first.stderr).contains("private-setting"));
    assert_eq!(fs::read_to_string(fixture.manifest()).unwrap(), source);
    assert!(!fixture.0.join("results").exists());
}

#[test]
fn table_and_json_show_the_same_schedule() {
    let json = command()
        .args(["run", "--dry-run", "--plan-format", "json", "--quiet"])
        .output()
        .unwrap();
    assert_status(&json, 0);
    let plan: Value = serde_json::from_slice(&json.stdout).unwrap();
    let mut cmd = command();
    cmd.args(["run", "--dry-run", "--plan-format", "table", "--quiet"]);
    let table = cmd.output().unwrap();
    assert_status(&table, 0);
    assert!(table.stderr.is_empty());
    assert_eq!(table.stdout, cmd.output().unwrap().stdout);
    let table = String::from_utf8(table.stdout).unwrap();
    assert!(table.contains(plan["id"].as_str().unwrap()));
    assert!(table.contains("Jobs: 1 warm-up, 3 measured"));
    assert!(table.contains("seed 42"));
    let lines: Vec<_> = table
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert_eq!(lines.len(), plan["jobs"].as_array().unwrap().len());
    for (line, job) in lines.iter().zip(plan["jobs"].as_array().unwrap()) {
        let cells: Vec<_> = line.split_whitespace().collect();
        assert_eq!(
            cells,
            [
                job["position"].to_string(),
                if job["warmup"].as_bool().unwrap() {
                    "warm-up"
                } else {
                    "measured"
                }
                .to_owned(),
                job["repetition"].to_string(),
                "mock".to_owned(),
                "sha256".to_owned(),
                "small".to_owned(),
                "default".to_owned(),
            ]
        );
    }
}

#[test]
fn dry_run_does_not_discover_adapters_or_write_files() {
    let fixture = Fixture::new();
    // Discovery would reject this file. Planning only records its path and hash.
    fs::write(
        fixture.0.join("mock.zkperf-adapter.json"),
        "not an adapter manifest",
    )
    .unwrap();
    let snapshot = || {
        let mut files: Vec<_> = fs::read_dir(&fixture.0)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.clone(), fs::read(path).unwrap())
            })
            .collect();
        files.sort();
        files
    };
    let before = snapshot();
    for format in ["json", "table"] {
        let output = command()
            .current_dir(&fixture.0)
            .args([
                "run",
                "--dry-run",
                "--plan-format",
                format,
                "--output-dir",
                "new-results",
            ])
            .output()
            .unwrap();
        assert_status(&output, 0);
        assert_eq!(snapshot(), before);
    }
}

#[test]
fn dry_run_never_launches_an_executable_adapter_command() {
    let fixture = Fixture::new();
    let marker = fixture.0.join("adapter-was-invoked");
    let adapter = serde_json::json!({
        "kind": "zkperf-adapter-manifest",
        "manifest_version": "1.0.0",
        "adapter_id": "mock",
        "display_name": "Execution canary",
        "protocol_versions": ["1.0.0"],
        "command": [env!("CARGO_BIN_EXE_zkperf"), "init", marker],
    });
    fs::write(
        fixture.0.join("mock.zkperf-adapter.json"),
        serde_json::to_vec(&adapter).unwrap(),
    )
    .unwrap();
    for format in ["json", "table"] {
        let output = command()
            .current_dir(&fixture.0)
            .args(["run", "--dry-run", "--plan-format", format])
            .output()
            .unwrap();
        assert_status(&output, 0);
        assert!(!marker.exists(), "dry-run invoked an adapter command");
    }
}

#[test]
fn planning_flags_are_run_only_and_conflicts_are_usage_errors() {
    for args in [
        vec!["validate", "--dry-run"],
        vec!["run", "--dry-run", "--print-config"],
        vec!["run", "--plan-format", "table"],
        vec!["run", "--dry-run", "--plan-format", "html"],
    ] {
        let output = command().args(args).output().unwrap();
        assert_status(&output, 2);
        assert!(output.stdout.is_empty());
    }
    let output = command().args(["run", "--help"]).output().unwrap();
    assert_status(&output, 0);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--dry-run"));
    assert!(help.contains("--plan-format"));
}

#[test]
fn dry_run_reports_invalid_configuration_and_plan_overflow() {
    for args in [
        vec!["run", "--dry-run", "--manifest", "missing.toml"],
        vec!["run", "--dry-run", "--warmups", "18446744073709551615"],
    ] {
        let output = command().args(args).output().unwrap();
        assert_status(&output, 3);
        assert!(output.stdout.is_empty());
    }
}
