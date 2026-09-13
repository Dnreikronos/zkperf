use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use process_wrap::std::{StdChildWrapper, StdCommandWrap};

use super::{AdapterInvocation, CancellationToken, OperationOutcome, OperationRecord};

pub(super) struct RawOutput {
    pub record: OperationRecord,
    pub stdout: Vec<u8>,
}

struct Guard {
    child: Box<dyn StdChildWrapper>,
    armed: bool,
}

impl Guard {
    fn stop(&mut self, record: &mut OperationRecord) {
        // Kill the containment group even if its leader already exited.
        let termination = self.child.start_kill();
        #[cfg(unix)]
        if termination.is_ok() && record.outcome == OperationOutcome::Success {
            record.outcome = OperationOutcome::ProcessFailed;
            record
                .errors
                .push("adapter left descendants running after closing its pipes".into());
        }
        if let Err(error) = termination {
            #[cfg(unix)]
            let absent = error.raw_os_error() == Some(3); // ESRCH: group already empty.
            #[cfg(not(unix))]
            let absent = false;
            if !absent {
                record.errors.push(format!("tree termination: {error}"));
            }
        }
        match self.child.wait() {
            Ok(status) => {
                record.exit_code = status.code();
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    record.signal = status.signal();
                }
            }
            Err(error) => record.errors.push(format!("reap: {error}")),
        }
        self.armed = false;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Covers I/O errors and unwinding as well as ordinary completion.
        if self.armed {
            drop(self.child.start_kill());
            drop(self.child.wait());
        }
    }
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
    error: Option<String>,
}

fn drain(mut source: impl Read, mut file: File, limit: usize, overflow: &AtomicBool) -> Capture {
    let mut capture = Capture::default();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                capture.error = Some(error.to_string());
                break;
            }
        };
        let retained = count.min(limit.saturating_sub(capture.bytes.len()));
        capture.bytes.extend_from_slice(&buffer[..retained]);
        if capture.error.is_none() {
            if let Err(error) = file.write_all(&buffer[..retained]) {
                capture.error = Some(error.to_string());
            }
        }
        capture.truncated |= retained < count;
        if capture.truncated {
            overflow.store(true, Ordering::Release);
        }
    }
    capture
}

pub(super) fn execute(
    invocation: &AdapterInvocation,
    cancellation: &CancellationToken,
    root: &Path,
    stdout: File,
    stderr: File,
    control: &cap_std::fs::Dir,
) -> RawOutput {
    let request = &invocation.request;
    let mut record = initial_record(request);
    if cancellation.is_cancelled() {
        record.outcome = OperationOutcome::Cancelled;
        return RawOutput {
            record,
            stdout: Vec::new(),
        };
    }
    let input = format!("{request}\n").into_bytes();
    let limit = Duration::from_nanos(request["timeout"]["limit_ns"].as_u64().unwrap());
    let grace = if invocation.graceful_cancellation {
        Duration::from_nanos(request["timeout"]["termination_grace_ns"].as_u64().unwrap())
    } else {
        Duration::ZERO
    };
    let mut command = command(invocation, root);
    let start = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => Guard { child, armed: true },
        Err(error) => {
            record.outcome = OperationOutcome::ProcessFailed;
            record.phase_duration_ns = nanos(start.elapsed());
            record.errors.push(format!("spawn: {error}"));
            return RawOutput {
                record,
                stdout: Vec::new(),
            };
        }
    };
    let mut resources = crate::resources::Sampler::start(child.child.inner().id(), start, limit);
    let mut stdin = child.child.stdin().take().unwrap();
    let out = child.child.stdout().take().unwrap();
    let err = child.child.stderr().take().unwrap();
    let overflow = AtomicBool::new(false);
    let stderr_overflow = AtomicBool::new(false);
    let (out, err, input_result) = thread::scope(|scope| {
        let mut child = child;
        let writer = scope.spawn(move || stdin.write_all(&input));
        let reader = scope.spawn(|| drain(out, stdout, invocation.limits.stdout_bytes, &overflow));
        let logger = scope.spawn(|| {
            drain(
                err,
                stderr,
                invocation.limits.stderr_bytes,
                &stderr_overflow,
            )
        });
        let mut watchdog = super::watchdog::Watchdog {
            record: &mut record,
            invocation,
            cancellation,
            control,
            overflow: &overflow,
            start,
            stopped: None,
            limit,
            grace,
            resources: &mut resources,
        };
        watchdog.wait(child.child.as_mut(), || {
            reader.is_finished() && logger.is_finished() && writer.is_finished()
        });
        let stopped = watchdog.stopped;
        let mut cleanup = stopped.unwrap_or_else(Instant::now);
        if stopped.is_some() {
            child.stop(&mut record);
        }
        let output = (
            reader.join().unwrap(),
            logger.join().unwrap(),
            writer.join().unwrap(),
        );
        if stopped.is_none() {
            complete_phase(
                &mut record,
                invocation,
                root,
                &output.0.bytes,
                start,
                &mut resources,
            );
            cleanup = Instant::now();
            child.stop(&mut record);
        }
        record.resources = resources.finish();
        record.cleanup_duration_ns = nanos(cleanup.elapsed());
        output
    });
    finish_capture(record, out, err, input_result)
}

fn finish_capture(
    mut record: OperationRecord,
    out: Capture,
    err: Capture,
    input_result: io::Result<()>,
) -> RawOutput {
    record.stdout_truncated = out.truncated;
    record.stderr_truncated = err.truncated;
    for error in out.error.into_iter().chain(err.error) {
        record.errors.push(format!("capture: {error}"));
    }
    if let Err(error) = input_result {
        record.errors.push(format!("stdin: {error}"));
    }
    if record.outcome == OperationOutcome::Success && !record.errors.is_empty() {
        record.outcome = OperationOutcome::ProcessFailed;
    }
    RawOutput {
        record,
        stdout: out.bytes,
    }
}

pub(super) fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn command(invocation: &AdapterInvocation, root: &Path) -> StdCommandWrap {
    let mut command = Command::new(&invocation.executable);
    command
        .args(&invocation.arguments)
        .env_clear()
        .envs(&invocation.environment)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = StdCommandWrap::from(command);
    #[cfg(unix)]
    command.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::std::JobObject);
    command
}

fn initial_record(request: &serde_json::Value) -> OperationRecord {
    OperationRecord {
        request_id: request["request_id"].as_str().unwrap().into(),
        operation: request["operation"].as_str().unwrap().into(),
        outcome: OperationOutcome::Success,
        exit_code: None,
        signal: None,
        phase_duration_ns: 0,
        cleanup_duration_ns: 0,
        resources: crate::ResourceEvidence::unavailable("not_spawned", "Adapter was not spawned."),
        stdout_truncated: false,
        stderr_truncated: false,
        errors: Vec::new(),
    }
}

fn complete_phase(
    record: &mut OperationRecord,
    invocation: &AdapterInvocation,
    root: &Path,
    bytes: &[u8],
    start: Instant,
    resources: &mut crate::resources::Sampler,
) {
    if record.outcome == OperationOutcome::Success {
        if let Err(error) = super::evidence::readable(root, bytes, invocation.limits) {
            record.outcome = OperationOutcome::ArtifactError;
            record.errors.push(error.to_string());
        }
    }
    let limit = invocation.request["timeout"]["limit_ns"].as_u64().unwrap();
    record.phase_duration_ns = nanos(resources.stop().duration_since(start));
    if record.phase_duration_ns >= limit {
        record.phase_duration_ns = limit;
        record.outcome = OperationOutcome::TimedOut;
    }
}
