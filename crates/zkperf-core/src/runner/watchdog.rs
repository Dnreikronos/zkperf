#[cfg(test)]
mod tests;

use super::{AdapterInvocation, CancellationToken, OperationOutcome, OperationRecord};
use process_wrap::std::StdChildWrapper;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub(super) struct Watchdog<'a> {
    pub record: &'a mut OperationRecord,
    pub invocation: &'a AdapterInvocation,
    pub cancellation: &'a CancellationToken,
    pub control: &'a cap_std::fs::Dir,
    pub overflow: &'a AtomicBool,
    pub start: Instant,
    pub stopped: Option<Instant>,
    pub limit: Duration,
    pub grace: Duration,
    pub resources: &'a mut crate::resources::Sampler,
}

impl Watchdog<'_> {
    pub fn wait(&mut self, child: &mut dyn StdChildWrapper, finished: impl Fn() -> bool) {
        loop {
            let exited = match exit_status(child, self.resources.ready()) {
                Ok(status) => status,
                Err(error) => {
                    self.resources.stop();
                    self.record.errors.push(format!("wait: {error}"));
                    self.record.outcome = OperationOutcome::ProcessFailed;
                    break;
                }
            };
            let elapsed = self.start.elapsed();
            if self.stopped.is_none() {
                let reason = if self.cancellation.is_cancelled() {
                    self.record.outcome = OperationOutcome::Cancelled;
                    Some("user_cancelled")
                } else if elapsed >= self.limit {
                    self.record.outcome = OperationOutcome::TimedOut;
                    Some("deadline_exceeded")
                } else if self.overflow.load(Ordering::Acquire) {
                    self.record.outcome = OperationOutcome::ProtocolError;
                    self.record
                        .errors
                        .push("protocol stdout exceeds byte limit".into());
                    Some("protocol_error")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    let stopped = self.resources.stop();
                    self.record.phase_duration_ns =
                        super::process::nanos(if reason == "deadline_exceeded" {
                            self.limit
                        } else {
                            stopped.duration_since(self.start)
                        });
                    self.stopped = Some(stopped);
                    if reason != "protocol_error" {
                        if let Err(error) =
                            super::evidence::cancel(self.control, &self.invocation.request, reason)
                        {
                            self.record
                                .errors
                                .push(format!("cancellation notice: {error}"));
                        }
                    }
                }
            }
            if let Some(status) = exited {
                self.record.exit_code = status.code;
                self.record.signal = status.signal;
                if finished() {
                    if self.stopped.is_none() && status.code != Some(0) {
                        self.record.outcome = OperationOutcome::ProcessFailed;
                    }
                    break;
                }
            }
            if let Some(stop) = self.stopped {
                if stop.elapsed() >= self.grace
                    || self.record.outcome == OperationOutcome::ProtocolError
                {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

struct ChildExit {
    code: Option<i32>,
    signal: Option<i32>,
}

fn exit_status(child: &mut dyn StdChildWrapper, ready: bool) -> std::io::Result<Option<ChildExit>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if !ready {
        use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};

        // Observe completion without reaping while the sampler anchors the PID.
        return waitid(
            WaitId::Pid(Pid::from_child(child.inner())),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map(|status| {
            status.map(|status| ChildExit {
                code: status.exit_status(),
                signal: status.terminating_signal(),
            })
        })
        .map_err(Into::into);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = ready;
    child.inner_mut().try_wait().map(|status| {
        status.map(|status| ChildExit {
            code: status.code(),
            #[cfg(unix)]
            signal: {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            },
            #[cfg(not(unix))]
            signal: None,
        })
    })
}
