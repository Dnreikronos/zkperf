//! Supervision of one adapter operation, with retained run evidence.

mod evidence;
mod process;
mod watchdog;
pub(crate) mod wire;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use serde_json::Value;

use crate::{Artifact, RunDirectory, RunError, RunWorkspace};

/// Cooperative harness cancellation, shared with a UI or signal handler.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Host limits, reduced by negotiated adapter limits after capabilities.
#[derive(Clone, Copy, Debug)]
pub struct RunnerLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub artifact_count: u64,
    pub artifact_bytes: u64,
    pub total_artifact_bytes: u64,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            stdout_bytes: 1 << 20,
            stderr_bytes: 1 << 20,
            artifact_count: 32,
            artifact_bytes: 1 << 30,
            total_artifact_bytes: 2 << 30,
        }
    }
}

/// One schema-valid request and its execution configuration.
///
/// `executable` must be absolute. Arguments are literal; the environment is
/// exhaustive and never implicitly inherits the harness environment. Input
/// artifacts must already exist beneath the operation root returned by
/// [`operation_path`]. The request uses `inputs`, `outputs`, and `control`.
#[derive(Clone, Debug)]
pub struct AdapterInvocation {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    pub request: Value,
    pub limits: RunnerLimits,
    pub graceful_cancellation: bool,
}

/// Distinguishes protocol completion from process and harness failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Success,
    Unsupported,
    AdapterError,
    ProcessFailed,
    ProtocolError,
    ArtifactError,
    TimedOut,
    Cancelled,
}

/// Persisted outcome. Durations are host monotonic nanoseconds.
#[derive(Clone, Debug, Serialize)]
pub struct OperationRecord {
    pub request_id: String,
    pub operation: String,
    pub outcome: OperationOutcome,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub phase_duration_ns: u64,
    pub cleanup_duration_ns: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub errors: Vec<String>,
}

/// Validated response and immutable artifact snapshots, plus the process record.
#[derive(Debug)]
pub struct OperationResult {
    pub record: OperationRecord,
    pub response: Option<Value>,
    pub artifacts: Vec<Artifact>,
}

/// The isolated operation root for a UUID request within an attempt.
///
/// # Errors
/// Returns an error for a noncanonical UUID, preventing path interpretation.
pub fn operation_path(workspace: &RunWorkspace, request_id: &str) -> Result<PathBuf, RunError> {
    wire::request_id(request_id)?;
    Ok(workspace.outputs().join(request_id))
}

/// Runs one subprocess and publishes its evidence, even on adapter failure.
///
/// # Errors
/// Returns an error for invalid host requests or unavailable evidence storage.
/// Adapter failures are returned as an [`OperationResult`].
pub fn run_operation(
    run: &mut RunDirectory,
    workspace: &RunWorkspace,
    invocation: &AdapterInvocation,
    cancellation: &CancellationToken,
) -> Result<OperationResult, RunError> {
    let validator = wire::Validator::new()?;
    validator.request(&invocation.request)?;
    if !invocation.executable.is_absolute() {
        return Err(wire::invalid("adapter executable must be absolute"));
    }
    let id = invocation.request["request_id"]
        .as_str()
        .ok_or_else(|| wire::invalid("request ID missing"))?;
    let root = operation_path(workspace, id)?;
    let evidence = evidence::Evidence::create(run, workspace, invocation, &root)?;
    let raw = process::execute(
        invocation,
        cancellation,
        &root,
        evidence.stdout,
        evidence.stderr,
        &evidence.control,
    );
    let mut result = OperationResult {
        record: raw.record,
        response: None,
        artifacts: Vec::new(),
    };
    if !raw.stdout.is_empty() && !result.record.stdout_truncated {
        match validator.response(&invocation.request, &raw.stdout) {
            Ok(response) => result.response = Some(response),
            Err(error) => {
                if result.record.outcome == OperationOutcome::Success {
                    result.record.outcome = OperationOutcome::ProtocolError;
                }
                result.record.errors.push(error);
            }
        }
    } else if result.record.outcome == OperationOutcome::Success {
        result.record.outcome = OperationOutcome::ProtocolError;
        result
            .record
            .errors
            .push("missing or oversized protocol stdout".into());
    }
    if matches!(
        result.record.outcome,
        OperationOutcome::Success | OperationOutcome::ArtifactError
    ) {
        if let Some(response) = &result.response {
            let artifacts_readable = result.record.outcome == OperationOutcome::Success;
            result.record.outcome = match response["status"].as_str().unwrap_or("error") {
                "success" => result.record.outcome,
                "unsupported" => OperationOutcome::Unsupported,
                _ => OperationOutcome::AdapterError,
            };
            if artifacts_readable {
                match evidence::artifacts(run, workspace, invocation, response) {
                    Ok(artifacts) => result.artifacts = artifacts,
                    Err(error) => {
                        if result.record.outcome == OperationOutcome::Success {
                            result.record.outcome = OperationOutcome::ArtifactError;
                        }
                        result.record.errors.push(error.to_string());
                    }
                }
            }
        }
    }
    evidence::finish(run, workspace, &result.record)?;
    Ok(result)
}
