use std::collections::HashSet;
use std::fs;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zkperf_core::{BenchmarkManifest, BenchmarkPlan, ManifestOverrides, PlanError};

const SOURCE: &str = include_str!("fixtures/manifest/zkperf.toml");

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-plan-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manifest");
        for file in [
            "workload.md",
            "input.bin",
            "output.bin",
            "mock.zkperf-adapter.json",
        ] {
            fs::copy(original.join(file), root.join(file)).unwrap();
        }
        fs::write(root.join("zkperf.toml"), source).unwrap();
        Self(root)
    }

    fn manifest(&self) -> BenchmarkManifest {
        BenchmarkManifest::load(self.0.join("zkperf.toml")).unwrap()
    }

    fn plan(&self) -> BenchmarkPlan {
        BenchmarkPlan::build(self.manifest()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn matrix_source() -> String {
    let (_, workload) = SOURCE.split_once("[[workloads]]").unwrap();
    let (workload, _) = workload.split_once("[[engines]]").unwrap();
    let (_, input) = workload.split_once("[[workloads.inputs]]").unwrap();
    let expanded = format!(
        "{workload}\n[[workloads.inputs]]\n{}",
        input.replace("small", "large")
    );
    let source = SOURCE.replace(workload, &expanded);
    let source = source.replace(
        "[[engines]]",
        &format!(
            "[[workloads]]\n{}\n[[engines]]",
            workload.replace("sha256", "sha512")
        ),
    );
    format!(
        "{source}\n[[engines]]\nid = 'beta'\nadapter = 'mock.zkperf-adapter.json'\nproof_modes = ['fast', 'slow']\n\n[[engines]]\nid = 'gamma'\nadapter = 'mock.zkperf-adapter.json'\nproof_modes = ['default']\n"
    )
}

#[test]
fn matrix_has_complete_unique_jobs_and_fixed_round_robin_order() {
    let fixture = Fixture::new(&matrix_source());
    let plan = fixture.plan();
    assert_eq!(plan.jobs().len(), 4 * 3 * 4);
    assert_eq!(plan.jobs().iter().filter(|job| job.warmup()).count(), 12);
    let ids: HashSet<_> = plan
        .jobs()
        .iter()
        .map(zkperf_core::PlannedJob::id)
        .collect();
    assert_eq!(ids.len(), plan.jobs().len());
    let engine_order: Vec<_> = plan.jobs()[12..24]
        .iter()
        .map(|job| job.engine_id().as_str())
        .collect();
    assert_eq!(
        engine_order,
        [
            "mock", "gamma", "beta", "beta", "gamma", "beta", "beta", "mock", "beta", "beta",
            "mock", "gamma"
        ]
    );
    let mut combinations = HashSet::new();
    for (position, job) in plan.jobs().iter().enumerate() {
        assert_eq!(job.position(), position as u64);
        assert_eq!(job.id(), format!("{}:{position}", plan.id()));
        assert_eq!(job.warmup(), position < 12);
        assert!(combinations.insert((
            job.engine_id().as_str(),
            job.workload_id().as_str(),
            job.input_id().as_str(),
            job.proof_mode().map(zkperf_core::Slug::as_str),
            job.warmup(),
            job.repetition()
        )));
        let workload = plan
            .manifest()
            .workloads()
            .iter()
            .find(|workload| workload.id() == job.workload_id())
            .unwrap();
        assert!(
            workload
                .inputs()
                .iter()
                .any(|input| input.id() == job.input_id())
        );
        assert_eq!(workload.phases().len(), 3);
    }
    for (offset, workload, input) in [
        (0, "sha256", "small"),
        (4, "sha256", "large"),
        (8, "sha512", "small"),
        (12, "sha256", "small"),
        (24, "sha256", "large"),
        (36, "sha512", "small"),
    ] {
        assert_eq!(plan.jobs()[offset].workload_id().as_str(), workload);
        assert_eq!(plan.jobs()[offset].input_id().as_str(), input);
    }
}

#[test]
fn identical_inputs_produce_identical_plans_and_seed_changes_order() {
    let fixture = Fixture::new(&matrix_source());
    let first = fixture.plan();
    let second = fixture.plan();
    assert_eq!(first, second);
    assert_eq!(
        first.normalized_debug().unwrap(),
        second.normalized_debug().unwrap()
    );
    fs::write(
        fixture.0.join("zkperf.toml"),
        matrix_source().replace("seed = 42", "seed = 43"),
    )
    .unwrap();
    let changed = fixture.plan();
    assert_ne!(first.id(), changed.id());
    let order = |plan: &BenchmarkPlan| {
        plan.jobs()
            .iter()
            .map(|job| job.engine_id().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_ne!(order(&first), order(&changed));
}

#[test]
fn provenance_retains_effective_settings_and_hashes_every_referenced_file() {
    let fixture = Fixture::new(SOURCE);
    let original = fixture.plan();
    assert_eq!(original.files().len(), 4);
    let effective = fixture
        .manifest()
        .with_overrides(ManifestOverrides {
            warmups: Some(0),
            runs: NonZeroU64::new(2),
            ..ManifestOverrides::default()
        })
        .unwrap();
    let plan = BenchmarkPlan::build(effective.clone()).unwrap();
    assert_eq!(plan.manifest(), &effective);
    assert_eq!(plan.jobs().len(), 2);
    assert!(plan.jobs().iter().all(|job| !job.warmup()));
    assert_ne!(plan.id(), original.id());
    for filename in [
        "workload.md",
        "input.bin",
        "output.bin",
        "mock.zkperf-adapter.json",
    ] {
        let path = fixture.0.join(filename).canonicalize().unwrap();
        let before = fs::read(&path).unwrap();
        fs::write(&path, b"changed file content").unwrap();
        let changed = fixture.plan();
        assert_ne!(original.id(), changed.id());
        let digest = |plan: &BenchmarkPlan| {
            plan.files()
                .iter()
                .find(|file| file.path() == path)
                .unwrap()
                .sha256()
                .clone()
        };
        assert_ne!(digest(&original), digest(&changed));
        fs::write(&path, before).unwrap();
    }
}

#[test]
fn execution_only_engine_without_proof_modes_still_has_jobs() {
    let source = SOURCE
        .replace("proof_modes = [\"default\"]", "proof_modes = []")
        .replace(
            "phases = [\"execution\", \"proving\", \"verification\"]",
            "phases = [\"execution\"]",
        );
    let fixture = Fixture::new(&source);
    let plan = fixture.plan();
    assert_eq!(plan.jobs().len(), 4);
    assert!(plan.jobs().iter().all(|job| job.proof_mode().is_none()));
    let json = plan.normalized_debug().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value["jobs"][0].get("proof_mode").is_none());
}

#[test]
fn overflowing_counts_and_unallocatable_plans_are_errors() {
    let fixture = Fixture::new(&matrix_source());
    for (warmups, runs) in [(u64::MAX, 1), (0, u64::MAX / 2), (0, u64::MAX / 12)] {
        let manifest = fixture
            .manifest()
            .with_overrides(ManifestOverrides {
                warmups: Some(warmups),
                runs: NonZeroU64::new(runs),
                ..ManifestOverrides::default()
            })
            .unwrap();
        assert!(matches!(
            BenchmarkPlan::build(manifest),
            Err(PlanError::TooManyJobs)
        ));
    }
}

#[test]
fn missing_files_after_loading_return_path_specific_errors() {
    let fixture = Fixture::new(SOURCE);
    let manifest = fixture.manifest();
    fs::remove_file(fixture.0.join("input.bin")).unwrap();
    let error = BenchmarkPlan::build(manifest).unwrap_err();
    assert!(matches!(error, PlanError::Fixture(_)));
    assert!(error.to_string().contains("input.bin"));
}

#[test]
fn diagnostic_plan_redacts_environment_and_does_not_embed_input_bytes() {
    let fixture = Fixture::new(&SOURCE.replace(
        "environment_variables = {}",
        "environment_variables = { BENCH_MODE = 'private-setting' }",
    ));
    fs::write(fixture.0.join("input.bin"), "private-input-bytes").unwrap();
    let plan = fixture.plan();
    let output = plan.normalized_debug().unwrap();
    assert!(output.contains("[redacted]"));
    assert!(!output.contains("private-setting"));
    assert!(!output.contains("private-input-bytes"));
}
