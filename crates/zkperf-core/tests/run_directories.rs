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
        drop(fs::remove_dir_all(&self.0));
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
    assert_eq!(
        record.environment_digest().value(),
        Some(&record.environment().value().unwrap().digest())
    );
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

#[test]
fn a_destination_that_appears_first_is_never_replaced() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    // Nothing recorded this path, so only publication can refuse it.
    let mut existing = run.open_new("artifacts/proof.bin").unwrap();
    existing.write_all(b"written first").unwrap();
    existing.flush().unwrap();

    let error = run
        .store(
            &request("artifacts/proof.bin", ArtifactKind::Proof),
            b"late",
        )
        .unwrap_err();

    assert!(matches!(error, RunError::AlreadyExists(_)), "{error}");
    assert_eq!(
        fs::read(run.path().join("artifacts").join("proof.bin")).unwrap(),
        b"written first"
    );
    assert!(run.artifacts().is_empty());
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

#[cfg(unix)]
#[test]
fn a_run_root_swapped_for_a_link_stops_every_write() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();

    let root = run.path().to_path_buf();
    fs::remove_dir_all(&root).unwrap();
    symlink(&outside, &root).unwrap();

    for error in [
        run.store(&request("logs/harness.log", ArtifactKind::Log), b"escaped")
            .unwrap_err(),
        run.open_new("logs/adapter.log").unwrap_err(),
        run.attempt(&plan.jobs()[0], 0).unwrap_err(),
    ] {
        assert!(matches!(error, RunError::SymbolicLink(_)), "{error}");
    }
    assert!(matches!(
        run.finish(RunOutcome::Completed),
        Err(RunError::SymbolicLink(_))
    ));
    assert_eq!(entries(&outside), [] as [String; 0]);
}

#[cfg(unix)]
#[test]
fn replacing_the_root_with_another_directory_is_detected() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();
    let root = run.path().to_path_buf();
    fs::rename(&root, fixture.0.join("original-run")).unwrap();
    fs::create_dir(&root).unwrap();

    assert!(matches!(
        run.store(&request("proof.bin", ArtifactKind::Proof), b"proof"),
        Err(RunError::Relocated(_))
    ));
    assert_eq!(entries(&root), [] as [String; 0]);
}

#[cfg(unix)]
#[test]
fn an_output_directory_linked_after_loading_cannot_host_a_run() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    // The manifest resolved "results" while it was still missing.
    symlink(&outside, fixture.results()).unwrap();

    let error = RunDirectory::create(&plan).unwrap_err();

    assert!(matches!(error, RunError::SymbolicLink(_)), "{error}");
    assert_eq!(entries(&outside), [] as [String; 0]);
}

#[test]
fn files_the_run_keeps_writing_cannot_become_artifacts() {
    let fixture = Fixture::new(SOURCE);
    let plan = fixture.plan();
    let mut run = RunDirectory::create(&plan).unwrap();

    for path in [
        "run.json",
        "plan.json",
        "artifacts.jsonl",
        "RUN.JSON",
        "Plan.Json",
        "ARTIFACTS.JSONL",
    ] {
        let adopted = run.adopt(&request(path, ArtifactKind::Other)).unwrap_err();
        let stored = run
            .store(&request(path, ArtifactKind::Other), b"replacement")
            .unwrap_err();
        assert!(matches!(adopted, RunError::InvalidPath { .. }), "{adopted}");
        assert!(matches!(stored, RunError::InvalidPath { .. }), "{stored}");
    }

    assert!(run.artifacts().is_empty());
    assert_eq!(
        fs::read_to_string(run.path().join("artifacts.jsonl")).unwrap(),
        ""
    );
}

#[cfg(unix)]
#[test]
fn adopting_fifo_without_writer_is_rejected() {
    use std::process::Command;
    use std::time::{Duration, Instant};

    const CHILD_MANIFEST: &str = "ZKPERF_TEST_FIFO_MANIFEST";
    if let Some(manifest) = std::env::var_os(CHILD_MANIFEST) {
        let plan = BenchmarkPlan::build(BenchmarkManifest::load(manifest).unwrap()).unwrap();
        let mut run = RunDirectory::create(&plan).unwrap();
        let fifo = run.path().join("proof.bin");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            run.adopt(&request("proof.bin", ArtifactKind::Proof)),
            Err(RunError::NotARegularFile(path)) if path == fifo
        ));
        assert!(run.artifacts().is_empty());
        assert!(
            fs::read(run.path().join("artifacts.jsonl"))
                .unwrap()
                .is_empty()
        );
        return;
    }

    let fixture = Fixture::new(SOURCE);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adopting_fifo_without_writer_is_rejected",
            "--nocapture",
        ])
        .env(CHILD_MANIFEST, fixture.0.join("zkperf.toml"))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "FIFO adoption child failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("adoption blocked waiting for a FIFO writer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
    assert_eq!(
        value["uri"],
        format!(".zkperf-artifacts/{}", value["id"].as_str().unwrap())
    );
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
fn adopted_snapshots_survive_producer_writes_and_path_replacement() {
    let fixture = Fixture::new(SOURCE);
    let mut run = RunDirectory::create(&fixture.plan()).unwrap();
    let mut producer = run.open_new("logs/adapter.log").unwrap();
    producer.write_all(b"original evidence").unwrap();
    let source = request("logs/adapter.log", ArtifactKind::Other);
    let artifact = run.adopt(&source).unwrap();
    let value = serde_json::to_value(&artifact).unwrap();
    let uri = value["uri"].as_str().unwrap();

    producer.write_all(b" later writes").unwrap();
    drop(producer);
    fs::remove_file(run.path().join(&source.path)).unwrap();
    fs::write(run.path().join(&source.path), b"replacement").unwrap();

    assert_eq!(
        fs::read(run.path().join(uri)).unwrap(),
        b"original evidence"
    );
    assert_eq!(value["digest"]["value"], sha256_hex(b"original evidence"));
    assert_eq!(value["byte_length"], b"original evidence".len());
    assert!(matches!(
        run.adopt(&source),
        Err(RunError::AlreadyExists(_))
    ));
    for reserved in [uri.to_owned(), uri.to_ascii_uppercase()] {
        let snapshot = request(&reserved, ArtifactKind::Other);
        assert!(matches!(
            run.open_new(&reserved),
            Err(RunError::InvalidPath { .. })
        ));
        assert!(matches!(
            run.store(&snapshot, b"overwrite"),
            Err(RunError::InvalidPath { .. })
        ));
        assert!(matches!(
            run.adopt(&snapshot),
            Err(RunError::InvalidPath { .. })
        ));
    }
    let index = fs::read_to_string(run.path().join("artifacts.jsonl")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(index.trim()).unwrap(),
        value
    );
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
    assert_eq!(
        record.environment_digest().value(),
        Some(&record.environment().value().unwrap().digest())
    );
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

#[test]
fn older_records_load_with_explicit_gaps_without_probing_the_readers_host() {
    let fixture = Fixture::new(SOURCE);
    let run = RunDirectory::create(&fixture.plan()).unwrap();
    let path = run.path().to_path_buf();
    run.finish(RunOutcome::Completed).unwrap();
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join("run.json")).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("environment");
    record.as_object_mut().unwrap().remove("environment_digest");
    fs::write(path.join("run.json"), serde_json::to_vec(&record).unwrap()).unwrap();
    let loaded = RunRecord::load(&path).unwrap();
    assert!(loaded.environment().value().is_none());
    assert!(loaded.environment_digest().value().is_none());
    let value = serde_json::to_value(loaded).unwrap();
    assert_eq!(value["environment"]["reason"]["code"], "not_recorded");
    assert_eq!(value["environment_digest"]["availability"], "unavailable");
}
