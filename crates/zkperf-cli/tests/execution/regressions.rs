use super::{adapter, run_directory};
use crate::support::{Fixture, assert_status, command};
use serde_json::Value;
use std::fs;
use std::process::Output;

fn run(fixture: &Fixture) -> Output {
    command()
        .args(["run", "--warmups", "0", "--runs", "1"])
        .current_dir(&fixture.0)
        .output()
        .unwrap()
}

fn assert_state(fixture: &Fixture, expected: &str) {
    let record: Value =
        serde_json::from_slice(&fs::read(run_directory(fixture).join("run.json")).unwrap())
            .unwrap();
    assert_eq!(record["state"], expected);
}

fn operations(fixture: &Fixture) -> Vec<String> {
    let mut operations = Vec::new();
    for attempt in fs::read_dir(run_directory(fixture).join("logs")).unwrap() {
        let path = attempt.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        for operation in fs::read_dir(path).unwrap() {
            let request: Value = serde_json::from_slice(
                &fs::read(operation.unwrap().path().join("request.json")).unwrap(),
            )
            .unwrap();
            let name = request["operation"].as_str().unwrap();
            operations.push(
                request["params"]["stage"]
                    .as_str()
                    .map_or_else(|| name.to_owned(), |stage| format!("{name}/{stage}")),
            );
        }
    }
    operations.sort();
    operations
}

#[test]
fn proving_receives_prepared_keys_and_parameters_in_each_fresh_workspace() {
    let fixture = Fixture::new();
    adapter(&fixture, "prepared-inputs");
    assert_status(&run(&fixture), 0);
    assert_state(&fixture, "completed");
}

#[test]
fn response_local_artifact_ids_do_not_replace_fixtures_or_prior_outputs() {
    let fixture = Fixture::new();
    adapter(&fixture, "colliding-artifacts");
    assert_status(&run(&fixture), 0);
    assert_state(&fixture, "completed");
}

#[test]
fn out_of_range_capability_limits_fail_without_panicking() {
    for limit in [
        "max_protocol_stdout_bytes",
        "max_artifact_count",
        "max_artifact_bytes",
        "max_total_artifact_bytes",
    ] {
        let fixture = Fixture::new();
        adapter(&fixture, &format!("large-limit-{limit}"));
        let output = run(&fixture);
        assert_status(&output, 6);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(limit), "{stderr}");
        assert_state(&fixture, "failed");
        assert_eq!(operations(&fixture), ["capabilities"]);
        assert!(
            run_directory(&fixture)
                .join("logs/execution-failure.json")
                .is_file()
        );
    }
}
