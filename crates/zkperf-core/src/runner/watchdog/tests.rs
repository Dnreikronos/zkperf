use super::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicU64;

use cap_std::{ambient_authority, fs::Dir};
use process_wrap::std::StdChild;

const CHILD_MODE: &str = "ZKPERF_TEST_WATCHDOG_CHILD";

#[test]
fn child_fixture() {
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        if mode == "sleep" {
            thread::sleep(Duration::from_secs(60));
        } else {
            std::process::exit(mode.parse().unwrap());
        }
    }
}

struct Fixture {
    child: StdChild,
    root: PathBuf,
    control: Option<Dir>,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-watchdog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let control = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "runner::watchdog::tests::child_fixture"])
            .env(CHILD_MODE, mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child: StdChild(child),
            root,
            control: Some(control),
        }
    }

    fn await_exit(&mut self) {
        let start = Instant::now();
        while exit_status(&mut self.child, false).unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn run(
        &mut self,
        cancellation: &CancellationToken,
        limit: Duration,
        finished: bool,
    ) -> OperationRecord {
        let invocation = AdapterInvocation {
            executable: PathBuf::new(),
            arguments: Vec::new(),
            environment: std::collections::BTreeMap::new(),
            request: serde_json::json!({"request_id": "test", "protocol": "zkperf-adapter", "protocol_version": "1.0.0"}),
            limits: crate::RunnerLimits::default(),
            graceful_cancellation: false,
        };
        let mut record = OperationRecord {
            request_id: "test".into(),
            operation: "test".into(),
            outcome: OperationOutcome::Success,
            exit_code: None,
            signal: None,
            phase_duration_ns: 0,
            cleanup_duration_ns: 0,
            resources: crate::ResourceEvidence::unavailable(
                "not_observed",
                "Test collector is pending.",
            ),
            stdout_truncated: false,
            stderr_truncated: false,
            errors: Vec::new(),
        };
        let overflow = AtomicBool::new(false);
        let mut resources = crate::resources::Sampler::pending_for_test();
        Watchdog {
            record: &mut record,
            invocation: &invocation,
            cancellation,
            control: self.control.as_ref().unwrap(),
            overflow: &overflow,
            start: Instant::now(),
            stopped: None,
            limit,
            grace: Duration::ZERO,
            resources: &mut resources,
        }
        .wait(&mut self.child, || finished);
        assert!(!resources.ready());
        record
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.child.0.kill());
        drop(self.child.0.wait());
        drop(self.control.take());
        drop(std::fs::remove_dir_all(&self.root));
    }
}

#[test]
fn exited_children_are_detected_while_sampler_bootstrap_is_pending() {
    for code in [0, 7] {
        let mut fixture = Fixture::new(&code.to_string());
        fixture.await_exit();
        let record = fixture.run(&CancellationToken::default(), Duration::from_secs(1), true);
        let expected = if code == 0 {
            OperationOutcome::Success
        } else {
            OperationOutcome::ProcessFailed
        };
        assert_eq!(record.outcome, expected, "{record:?}");
        assert_eq!(record.exit_code, Some(code));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            let status = waitid(
                WaitId::Pid(Pid::from_child(&fixture.child.0)),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .unwrap()
            .expect("the root must remain waitable until sampling stops");
            assert_eq!(status.exit_status(), Some(code));
        }
        assert_eq!(fixture.child.0.wait().unwrap().code(), Some(code));
    }
}

#[test]
fn pending_bootstrap_still_obeys_deadlines_and_cancellation() {
    for cancelled in [false, true] {
        let mut fixture = Fixture::new("sleep");
        let token = CancellationToken::default();
        if cancelled {
            token.cancel();
        }
        let record = fixture.run(&token, Duration::ZERO, true);
        let expected = if cancelled {
            OperationOutcome::Cancelled
        } else {
            OperationOutcome::TimedOut
        };
        assert_eq!(record.outcome, expected);
        assert!(fixture.root.join("cancel.json").is_file());
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn signal_exit_is_preserved_while_sampler_bootstrap_is_pending() {
    let mut fixture = Fixture::new("sleep");
    fixture.child.0.kill().unwrap();
    fixture.await_exit();
    let record = fixture.run(&CancellationToken::default(), Duration::from_secs(1), true);
    assert_eq!(record.outcome, OperationOutcome::ProcessFailed);
    assert_eq!(record.exit_code, None);
    assert_eq!(record.signal, Some(9));
}

#[test]
fn unfinished_pipes_still_timeout_after_the_child_exits() {
    let mut fixture = Fixture::new("0");
    fixture.await_exit();
    let record = fixture.run(&CancellationToken::default(), Duration::ZERO, false);
    assert_eq!(record.outcome, OperationOutcome::TimedOut);
    assert_eq!(record.exit_code, Some(0));
}
