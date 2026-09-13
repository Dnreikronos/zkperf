#[path = "subprocess_runner/regressions.rs"]
mod regressions;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use zkperf_core::{
    AdapterInvocation, BenchmarkManifest, BenchmarkPlan, CancellationToken, OperationOutcome,
    OperationResult, RunDirectory, RunWorkspace, RunnerLimits,
};

struct Fixture {
    root: PathBuf,
    run: Option<RunDirectory>,
    workspace: RunWorkspace,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-supervisor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manifest");
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
        }
        let plan = BenchmarkPlan::build(BenchmarkManifest::load(root.join("zkperf.toml")).unwrap())
            .unwrap();
        // Host collection and executable hashing are setup, not subprocess time.
        let run = RunDirectory::create(&plan).unwrap();
        let workspace = run.attempt(&plan.jobs()[0], 0).unwrap();
        Self {
            root,
            run: Some(run),
            workspace,
        }
    }

    fn invocation(&self, mode: &str) -> AdapterInvocation {
        let python = Command::new(if cfg!(windows) { "python" } else { "python3" })
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(python.status.success());
        let source = Path::new(env!("CARGO_MANIFEST_DIR"));
        let catalog: Value =
            serde_json::from_str(include_str!("../../../examples/protocol-v1.json")).unwrap();
        AdapterInvocation {
            executable: PathBuf::from(String::from_utf8(python.stdout).unwrap().trim()),
            arguments: vec![
                source.join("tests/fixtures/runner.py").into_os_string(),
                mode.into(),
                "$(echo unexpanded)".into(),
                source
                    .join("../../examples/protocol-v1.json")
                    .into_os_string(),
            ],
            environment: BTreeMap::from([
                ("EXPLICIT_VALUE".into(), "literal $VALUE".into()),
                ("PID_FILE".into(), self.root.join("pid").into_os_string()),
                (
                    "SURVIVOR_FILE".into(),
                    self.root.join("survived").into_os_string(),
                ),
            ]),
            request: catalog["exchanges"][0]["request"].clone(),
            limits: RunnerLimits::default(),
            graceful_cancellation: false,
        }
    }

    fn run(
        &mut self,
        invocation: &AdapterInvocation,
        token: &CancellationToken,
    ) -> (OperationResult, PathBuf) {
        let mut run = self.run.take().expect("fixture runs one operation");
        let workspace = &self.workspace;
        let path = run.path().to_path_buf();
        let result = zkperf_core::run_operation(&mut run, workspace, invocation, token).unwrap();
        let evidence = path
            .join("logs")
            .join(workspace.attempt_id().to_string())
            .join(result.record.request_id.clone());
        assert!(evidence.join("request.json").is_file());
        let record: Value =
            serde_json::from_slice(&fs::read(evidence.join("outcome.json")).unwrap()).unwrap();
        assert_eq!(record, serde_json::to_value(&result.record).unwrap());
        (result, evidence)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.run.take());
        drop(fs::remove_dir_all(&self.root));
    }
}

#[test]
fn success_uses_isolated_cwd_literal_arguments_and_explicit_environment() {
    let mut fixture = Fixture::new();
    let (result, evidence) = fixture.run(
        &fixture.invocation("environment"),
        &CancellationToken::default(),
    );
    assert_eq!(
        result.record.outcome,
        OperationOutcome::Success,
        "{:?}",
        result.record
    );
    assert_eq!(result.record.exit_code, Some(0));
    assert!(result.record.phase_duration_ns > 0);
    assert_eq!(
        fs::read(evidence.join("stderr.bin")).unwrap(),
        b"adapter diagnostic\n"
    );
}

#[test]
fn process_protocol_and_structured_failures_are_distinct_and_persist_logs() {
    for (mode, expected) in [
        ("exit", OperationOutcome::ProcessFailed),
        ("malformed", OperationOutcome::ProtocolError),
        ("mismatch", OperationOutcome::ProtocolError),
        ("invalid-utf8", OperationOutcome::ProtocolError),
        ("trailing", OperationOutcome::ProtocolError),
        ("error", OperationOutcome::AdapterError),
        ("unsupported", OperationOutcome::Unsupported),
    ] {
        let mut fixture = Fixture::new();
        let (result, evidence) =
            fixture.run(&fixture.invocation(mode), &CancellationToken::default());
        assert_eq!(
            result.record.outcome, expected,
            "{mode}: {:?}",
            result.record
        );
        assert!(!fs::read(evidence.join("stderr.bin")).unwrap().is_empty());
        if mode == "exit" {
            assert_eq!(result.record.exit_code, Some(7));
        }
    }
}

#[test]
fn flooding_stderr_is_bounded_and_does_not_corrupt_protocol() {
    let mut fixture = Fixture::new();
    let mut invocation = fixture.invocation("flood");
    invocation.limits.stderr_bytes = 4096;
    let (result, evidence) = fixture.run(&invocation, &CancellationToken::default());
    assert_eq!(result.record.outcome, OperationOutcome::Success);
    assert!(result.record.stderr_truncated);
    assert_eq!(
        fs::metadata(evidence.join("stderr.bin")).unwrap().len(),
        4096
    );
}

#[test]
fn oversized_stdout_terminates_without_waiting_for_deadline() {
    let mut fixture = Fixture::new();
    let mut invocation = fixture.invocation("overflow");
    invocation.limits.stdout_bytes = 4096;
    let start = Instant::now();
    let (result, evidence) = fixture.run(&invocation, &CancellationToken::default());
    assert_eq!(result.record.outcome, OperationOutcome::ProtocolError);
    assert!(result.record.stdout_truncated);
    assert_eq!(
        fs::metadata(evidence.join("stdout.bin")).unwrap().len(),
        4096
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn timeout_covers_blocked_stdin_and_inherited_pipes() {
    for mode in ["blocked-input", "tree", "orphan-pipes"] {
        let mut fixture = Fixture::new();
        let mut invocation = fixture.invocation(mode);
        invocation.request["timeout"]["limit_ns"] = 300_000_000_u64.into();
        if mode == "blocked-input" {
            invocation.request["operation"] = "metadata".into();
            invocation.request["params"] = serde_json::json!({"configuration": {"padding": "X".repeat(1024 * 1024)}, "artifacts": []});
        }
        let start = Instant::now();
        let (result, _) = fixture.run(&invocation, &CancellationToken::default());
        assert_eq!(
            result.record.outcome,
            OperationOutcome::TimedOut,
            "{mode}: {:?}",
            result.record
        );
        assert_eq!(result.record.phase_duration_ns, 300_000_000);
        assert!(start.elapsed() < Duration::from_secs(5));
        if mode != "blocked-input" {
            assert!(fixture.root.join("pid").exists());
            std::thread::sleep(Duration::from_millis(2100));
            assert!(
                !fixture.root.join("survived").exists(),
                "descendant survived {mode}"
            );
        }
    }
}

#[test]
fn cancellation_is_distinct_and_grace_is_excluded_from_phase_timing() {
    let mut fixture = Fixture::new();
    let mut invocation = fixture.invocation("graceful");
    invocation.graceful_cancellation = true;
    let token = CancellationToken::default();
    let cancel = token.clone();
    let pid = fixture.root.join("pid");
    let trigger = std::thread::spawn(move || {
        let start = Instant::now();
        while !pid.exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.cancel();
    });
    let (result, _) = fixture.run(&invocation, &token);
    trigger.join().unwrap();
    assert_eq!(result.record.outcome, OperationOutcome::Cancelled);
    assert!(result.record.cleanup_duration_ns >= 150_000_000);
    assert_eq!(result.record.exit_code, Some(0));
    assert!(result.response.is_some());
}

#[cfg(unix)]
#[test]
fn signal_exit_is_recorded() {
    let mut fixture = Fixture::new();
    let (result, _) = fixture.run(&fixture.invocation("signal"), &CancellationToken::default());
    assert_eq!(result.record.outcome, OperationOutcome::ProcessFailed);
    assert_eq!(result.record.signal, Some(15));
    assert_eq!(result.record.exit_code, None);
}

#[test]
fn timeout_stays_timed_out_when_adapter_returns_during_grace() {
    let mut fixture = Fixture::new();
    let mut invocation = fixture.invocation("graceful");
    invocation.graceful_cancellation = true;
    invocation.request["timeout"]["limit_ns"] = 500_000_000_u64.into();
    let (result, _) = fixture.run(&invocation, &CancellationToken::default());
    assert_eq!(result.record.outcome, OperationOutcome::TimedOut);
    assert_eq!(result.record.phase_duration_ns, 500_000_000);
    assert!(result.record.cleanup_duration_ns >= 150_000_000);
    assert_eq!(result.record.exit_code, Some(0));
}

#[test]
fn cancelled_before_spawn_retains_a_zero_duration_outcome() {
    let mut fixture = Fixture::new();
    let token = CancellationToken::default();
    token.cancel();
    let mut invocation = fixture.invocation("tree");
    invocation.executable = fixture.root.join("must-not-spawn");
    let (result, _) = fixture.run(&invocation, &token);
    assert_eq!(result.record.outcome, OperationOutcome::Cancelled);
    assert_eq!(result.record.exit_code, None);
    assert_eq!(result.record.phase_duration_ns, 0);
    assert!(!fixture.root.join("pid").exists());
}

#[test]
fn duplicate_invocations_cannot_overwrite_existing_evidence() {
    let mut fixture = Fixture::new();
    let invocation = fixture.invocation("environment");
    let mut run = fixture.run.take().unwrap();
    let workspace = &fixture.workspace;
    let token = CancellationToken::default();
    let first = zkperf_core::run_operation(&mut run, workspace, &invocation, &token).unwrap();
    assert_eq!(first.record.outcome, OperationOutcome::Success);
    assert!(zkperf_core::run_operation(&mut run, workspace, &invocation, &token).is_err());
}

#[cfg(unix)]
#[test]
fn silent_descendants_are_terminated_after_the_leader_exits() {
    let mut fixture = Fixture::new();
    let start = Instant::now();
    let (result, _) = fixture.run(
        &fixture.invocation("orphan-silent"),
        &CancellationToken::default(),
    );
    assert_eq!(result.record.outcome, OperationOutcome::ProcessFailed);
    assert!(
        result
            .record
            .errors
            .iter()
            .any(|message| message.contains("descendants"))
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(2100));
    assert!(!fixture.root.join("survived").exists());
}
