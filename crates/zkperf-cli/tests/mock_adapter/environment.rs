use super::{assert_status, command, configure, fixture, operations, run_path};
use serde_json::Value;
use std::fs;
use std::path::Path;

fn read(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

pub(super) fn assert_capture(root: &Path) {
    let record = zkperf_core::RunRecord::load(root).unwrap();
    assert_eq!(
        record.environment_digest().value(),
        Some(&record.environment().value().unwrap().digest())
    );
    let value = read(root.join("run.json"));
    assert!(value["environment"]["host"]["operating_system"].is_object());
    assert!(
        value["environment"]["harness"]["rustc"]
            .as_str()
            .unwrap()
            .starts_with("rustc ")
    );
    assert!(root.join("metadata/0000000000/initial.json").is_file());
}

#[test]
fn startup_and_preparation_failures_keep_the_metadata_already_observed() {
    for target in ["capabilities", "metadata", "prepare.setup"] {
        let fixture = fixture();
        configure(&fixture, "error", target);
        let output = command()
            .args(["run", "--warmups", "0", "--runs", "1", "--manifest"])
            .arg(fixture.0.join("mock benchmark/zkperf.toml"))
            .output()
            .unwrap();
        assert_status(&output, 6);
        let root = run_path(&fixture);
        assert_capture(&root);
        let initial = read(root.join("metadata/0000000000/initial.json"));
        assert_eq!(initial["data"]["requested_proof_mode"], "mock-core");
        assert_eq!(
            initial["data"]["identity"]["sdk"]["availability"],
            "unavailable"
        );
        assert_eq!(
            initial["data"]["guest"]["artifacts"]["availability"],
            "unavailable"
        );
        if target == "prepare.setup" {
            let build = read(root.join("metadata/0000000000/build.json"));
            assert!(build["data"]["identity"]["adapter"]["version"].is_string());
            assert!(build["data"]["guest"]["artifacts"].is_array());
            assert!(!root.join("metadata/0000000000/prepared.json").exists());
        }
        if target == "metadata" {
            let capabilities = read(root.join("metadata/0000000000/capabilities.json"));
            assert!(capabilities["data"]["identity"]["adapter"]["version"].is_string());
        }
    }
}

#[test]
fn repeated_runs_keep_versions_and_guest_hashes_stable_and_exclude_environment_secrets() {
    let fixture = fixture();
    let manifest = fixture.0.join("mock benchmark/zkperf.toml");
    let source = fs::read_to_string(&manifest).unwrap().replace(
        "environment_variables = {}",
        "environment_variables = { RAYON_NUM_THREADS = '2', OMP_NUM_THREADS = 'invalid-private-value', CUSTOM_SETTING = 'private-token' }",
    );
    fs::write(&manifest, source).unwrap();
    for _ in 0..2 {
        let output = command()
            .args(["run", "--warmups", "0", "--runs", "1", "--manifest"])
            .arg(&manifest)
            .env("PRIVATE_PARENT_SECRET", "inherited-secret")
            .env("MKL_NUM_THREADS", "33")
            .output()
            .unwrap();
        assert_status(&output, 0);
    }
    let mut captures = Vec::new();
    for entry in fs::read_dir(fixture.0.join("mock benchmark/results")).unwrap() {
        let root = entry.unwrap().path();
        assert_capture(&root);
        let record = read(root.join("run.json"));
        let environment = &record["environment"]["environment_variables"];
        assert_eq!(environment["RAYON_NUM_THREADS"], "2");
        assert_eq!(
            environment["OMP_NUM_THREADS"]["availability"],
            "unavailable"
        );
        assert!(environment.get("MKL_NUM_THREADS").is_none());
        let prepared = read(root.join("metadata/0000000000/prepared.json"));
        let data = &prepared["data"];
        assert_eq!(data["identity"]["sdk"]["status"], "unavailable");
        assert_eq!(data["identity"]["toolchain"]["name"], "python");
        assert!(data["identity"]["backend"]["version"].is_string());
        assert_eq!(data["proof_mode"]["id"], "mock-core");
        assert_eq!(
            data["guest"]["source_revision"]["availability"],
            "unavailable"
        );
        assert_eq!(data["guest"]["artifacts"].as_array().unwrap().len(), 1);
        let metadata_calls: Vec<_> = operations(&root)
            .into_iter()
            .filter(|(request, _)| request["operation"] == "metadata")
            .collect();
        assert_eq!(metadata_calls.len(), 2);
        assert!(
            metadata_calls
                .iter()
                .any(|(request, _)| request["params"]["artifacts"].as_array().unwrap().len() == 3)
        );
        for source in [
            record.to_string(),
            prepared.to_string(),
            fs::read_to_string(root.join("plan.json")).unwrap(),
        ] {
            for secret in ["invalid-private-value", "private-token", "inherited-secret"] {
                assert!(!source.contains(secret));
            }
        }
        captures.push((
            record["environment_digest"].clone(),
            prepared["digest"].clone(),
        ));
    }
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0], captures[1]);
}
