use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use zkperf_core::{
    ArtifactKind, ArtifactRequest, BenchmarkManifest, BenchmarkPlan, NonEmptyString, RunDirectory,
    RunError, RunOutcome, RunRecord, RunState,
};

const SOURCE: &str = include_str!("fixtures/manifest/zkperf.toml");

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-run-{}-{}",
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

    fn plan(&self) -> BenchmarkPlan {
        BenchmarkPlan::build(BenchmarkManifest::load(self.0.join("zkperf.toml")).unwrap()).unwrap()
    }

    fn results(&self) -> PathBuf {
        self.0.join("results")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn request(path: &str, kind: ArtifactKind) -> ArtifactRequest {
    ArtifactRequest {
        path: path.to_owned(),
        name: NonEmptyString::new("artifact").unwrap(),
        kind,
        media_type: "application/octet-stream".to_owned(),
        attempt_id: None,
    }
}

fn sha256_hex(contents: &[u8]) -> String {
    let mut encoded = String::new();
    for byte in Sha256::digest(contents) {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn entries(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn runs_never_overwrite_one_another() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();

    let mut first = RunDirectory::create(&plan).unwrap();
    first
        .store(&request("reports/report.json", ArtifactKind::Report), b"{}")
        .unwrap();
    let second = RunDirectory::create(&plan).unwrap();

    assert_ne!(first.id(), second.id());
    assert_ne!(first.path(), second.path());
    assert!(
        second
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&second.id().to_string())
    );
    assert_eq!(entries(&fixture.results()).len(), 2);
    assert_eq!(
        fs::read(first.path().join("reports").join("report.json")).unwrap(),
        b"{}"
    );

    let error = first
        .store(
            &request("reports/report.json", ArtifactKind::Report),
            b"replacement",
        )
        .unwrap_err();
    assert!(matches!(error, RunError::AlreadyExists(_)));
    assert_eq!(
        fs::read(first.path().join("reports").join("report.json")).unwrap(),
        b"{}"
    );
}

#[test]
fn interrupted_runs_remain_diagnosable() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();

    let path = {
        let mut run = RunDirectory::create(&plan).unwrap();
        let mut log = run.open_new("logs/adapter.log").unwrap();
        log.write_all(b"proving started\n").unwrap();
        log.flush().unwrap();
        run.store(
            &request("logs/harness.log", ArtifactKind::Log),
            b"planned\n",
        )
        .unwrap();
        run.path().to_path_buf()
    };

    let record = RunRecord::load(&path).unwrap();
    assert_eq!(record.state(), RunState::InProgress);
    assert!(record.finished_at().is_none());
    assert_eq!(record.plan_id(), plan.id());
    assert!(path.join("plan.json").is_file());
    assert_eq!(
        fs::read_to_string(path.join("logs").join("adapter.log")).unwrap(),
        "proving started\n"
    );

    let index = fs::read_to_string(path.join("artifacts.jsonl")).unwrap();
    assert_eq!(index.lines().count(), 1);
    let recorded: serde_json::Value = serde_json::from_str(index.lines().next().unwrap()).unwrap();
    assert_eq!(recorded["uri"], "logs/harness.log");
    assert_eq!(recorded["digest"]["value"], sha256_hex(b"planned\n"));
}

#[test]
fn artifact_paths_cannot_escape_the_run_directory() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    for path in [
        "",
        ".",
        "..",
        "../escape.bin",
        "logs/../../escape.bin",
        "/escape.bin",
        "logs//escape.bin",
        "logs/",
        "logs\\escape.bin",
        "escape artifact.bin",
        "nul",
        "logs/COM1.log",
        "logs/escape.bin.",
    ] {
        let error = run
            .store(&request(path, ArtifactKind::Other), b"escaped")
            .unwrap_err();
        assert!(
            matches!(error, RunError::InvalidPath { .. }),
            "{path} must be rejected, got {error}"
        );
    }

    assert!(!fixture.0.join("escape.bin").exists());
    assert!(!fixture.results().join("escape.bin").exists());
    assert_eq!(
        entries(run.path()),
        ["artifacts.jsonl", "plan.json", "run.json"]
    );
}

#[test]
fn rejected_requests_leave_no_evidence_behind() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    let error = run
        .store(
            &ArtifactRequest {
                path: "reports/report.json".to_owned(),
                name: NonEmptyString::new("report").unwrap(),
                kind: ArtifactKind::Report,
                media_type: "not-a-media-type".to_owned(),
                attempt_id: None,
            },
            b"{}",
        )
        .unwrap_err();

    assert!(matches!(error, RunError::Artifact(_)), "{error}");
    assert!(!run.path().join("reports").exists());
    assert!(run.artifacts().is_empty());
    assert!(
        fs::read_to_string(run.path().join("artifacts.jsonl"))
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        run.adopt(&request("artifacts/absent.bin", ArtifactKind::Proof)),
        Err(RunError::Io { .. })
    ));
}

#[cfg(unix)]
#[test]
fn symbolic_links_inside_a_run_directory_are_refused() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();

    symlink(&outside, run.path().join("logs")).unwrap();
    symlink(outside.join("proof.bin"), run.path().join("proof.bin")).unwrap();

    for path in ["logs/escape.log", "proof.bin"] {
        let error = run
            .store(&request(path, ArtifactKind::Other), b"escaped")
            .unwrap_err();
        assert!(
            matches!(error, RunError::SymbolicLink(_)),
            "{path} must be rejected, got {error}"
        );
    }
    assert!(matches!(
        run.adopt(&request("proof.bin", ArtifactKind::Proof)),
        Err(RunError::SymbolicLink(_))
    ));
    assert_eq!(entries(&outside), [] as [String; 0]);
}

#[test]
fn stored_and_adopted_artifacts_carry_report_integrity_hashes() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    let stored = run
        .store(
            &ArtifactRequest {
                path: "artifacts/proof.bin".to_owned(),
                name: NonEmptyString::new("proof").unwrap(),
                kind: ArtifactKind::Proof,
                media_type: "application/octet-stream".to_owned(),
                attempt_id: None,
            },
            b"proof-bytes",
        )
        .unwrap();
    let value = serde_json::to_value(&stored).unwrap();
    assert_eq!(value["uri"], "artifacts/proof.bin");
    assert_eq!(value["kind"], "proof");
    assert_eq!(value["byte_length"], 11);
    assert_eq!(value["digest"]["algorithm"], "sha256");
    assert_eq!(value["digest"]["value"], sha256_hex(b"proof-bytes"));
    assert!(value.get("attempt_id").is_none());

    let workspace = run.attempt(&plan.jobs()[0], 0).unwrap();
    let produced = b"adapter-written-proof";
    fs::write(workspace.outputs().join("proof.bin"), produced).unwrap();
    let adopted = run
        .adopt(&ArtifactRequest {
            path: workspace.output_path("proof.bin"),
            name: NonEmptyString::new("proof").unwrap(),
            kind: ArtifactKind::Proof,
            media_type: "application/octet-stream".to_owned(),
            attempt_id: Some(workspace.attempt_id()),
        })
        .unwrap();
    let value = serde_json::to_value(&adopted).unwrap();
    assert_eq!(value["uri"], "attempts/0000000000-0000/outputs/proof.bin");
    assert_eq!(value["byte_length"], produced.len());
    assert_eq!(value["digest"]["value"], sha256_hex(produced));
    assert_eq!(value["attempt_id"], workspace.attempt_id().to_string());

    assert_eq!(run.artifacts().len(), 2);
    let identities: HashSet<_> = run
        .artifacts()
        .iter()
        .map(|artifact| serde_json::to_value(artifact).unwrap()["id"].clone())
        .collect();
    assert_eq!(identities.len(), 2);
    assert!(matches!(
        run.adopt(&request("artifacts/proof.bin", ArtifactKind::Proof)),
        Err(RunError::AlreadyExists(_))
    ));
}

#[test]
fn attempt_workspaces_are_isolated_and_never_reused() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    let first = run.attempt(&plan.jobs()[0], 0).unwrap();
    let replacement = run.attempt(&plan.jobs()[0], 1).unwrap();
    let other_job = run.attempt(&plan.jobs()[1], 0).unwrap();

    for workspace in [&first, &replacement, &other_job] {
        assert!(workspace.root().starts_with(run.path()));
        assert!(workspace.inputs().is_dir());
        assert!(workspace.outputs().is_dir());
        assert!(workspace.control().is_dir());
        assert!(!workspace.inputs().starts_with(workspace.outputs()));
        assert!(!workspace.outputs().starts_with(workspace.control()));
    }

    let roots: HashSet<_> = [&first, &replacement, &other_job]
        .iter()
        .map(|workspace| workspace.root().to_path_buf())
        .collect();
    let attempts: HashSet<_> = [&first, &replacement, &other_job]
        .iter()
        .map(|workspace| workspace.attempt_id())
        .collect();
    assert_eq!(roots.len(), 3);
    assert_eq!(attempts.len(), 3);

    assert!(matches!(
        run.attempt(&plan.jobs()[0], 0),
        Err(RunError::AlreadyExists(_))
    ));
    assert_eq!(
        first.output_path("proof.bin"),
        "attempts/0000000000-0000/outputs/proof.bin"
    );
    assert_eq!(
        first.input_path("canonical-input.bin"),
        "attempts/0000000000-0000/inputs/canonical-input.bin"
    );
    assert_eq!(
        first.control_path("cancel"),
        "attempts/0000000000-0000/control/cancel"
    );

    run.store(
        &request(
            &first.input_path("canonical-input.bin"),
            ArtifactKind::Other,
        ),
        b"canonical-input",
    )
    .unwrap();
    assert!(first.inputs().join("canonical-input.bin").is_file());
}

#[test]
fn finished_runs_publish_their_outcome_and_provenance() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();
    run.store(&request("logs/harness.log", ArtifactKind::Log), b"done\n")
        .unwrap();
    let path = run.path().to_path_buf();

    let record = run.finish(RunOutcome::Completed).unwrap();

    assert_eq!(record.state(), RunState::Completed);
    assert!(
        record
            .started_at()
            .precedes_or_equals(record.finished_at().unwrap())
    );
    assert_eq!(record.artifacts().get(), 1);
    assert_eq!(record.plan_id(), plan.id());
    assert_eq!(record.manifest_path(), plan.manifest().manifest_path());
    assert_eq!(
        record.manifest_digest().value(),
        sha256_hex(&fs::read(fixture.0.join("zkperf.toml")).unwrap())
    );
    assert_eq!(record.files(), plan.files());
    assert!(!record.files().is_empty());
    assert_eq!(RunRecord::load(&path).unwrap(), record);

    let failed = RunDirectory::create(&plan)
        .unwrap()
        .finish(RunOutcome::Failed)
        .unwrap();
    assert_eq!(failed.state(), RunState::Failed);
}

#[test]
fn canonical_writes_leave_no_partial_or_temporary_files() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();
    run.store(&request("reports/report.json", ArtifactKind::Report), b"{}")
        .unwrap();
    let path = run.path().to_path_buf();
    run.finish(RunOutcome::Completed).unwrap();

    for directory in [path.clone(), path.join("reports")] {
        assert!(
            !entries(&directory)
                .iter()
                .any(|name| name.starts_with(".zkperf-")),
            "{} kept a temporary file",
            directory.display()
        );
    }
    serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path.join("run.json")).unwrap())
        .unwrap();
}

#[test]
fn persisted_evidence_keeps_configured_secrets_redacted() {
    let fixture = Fixture::new(&SOURCE.replace(
        "environment_variables = {}",
        "environment_variables = { BENCH_MODE = 'private-setting' }",
    ));
    let plan = fixture.plan();
    let run = RunDirectory::create(&plan).unwrap();

    let snapshot = fs::read_to_string(run.path().join("plan.json")).unwrap();
    assert!(snapshot.contains("[redacted]"));
    assert!(!snapshot.contains("private-setting"));
    assert!(snapshot.contains(plan.id()));
}
