//! Sequential execution of the jobs in a validated plan.

mod capabilities;
mod session;
mod stages;

use std::path::PathBuf;

use crate::{BenchmarkPlan, CancellationToken, RunDirectory, RunError, RunOutcome, RunRecord};

/// Retained run location and final state, including failures and cancellation.
#[derive(Debug)]
pub struct ExecutionResult {
    pub directory: PathBuf,
    pub record: RunRecord,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// Executes planned jobs in order, stopping on the first failed operation.
///
/// Produces operation evidence; statistical/report production is separate.
///
/// # Errors
/// Returns an error if the run cannot be created or its final state published.
pub fn execute_plan(
    plan: &BenchmarkPlan,
    cancellation: &CancellationToken,
) -> Result<ExecutionResult, RunError> {
    let mut run = RunDirectory::create(plan)?;
    let directory = run.path().to_path_buf();
    let mut error = (plan
        .manifest()
        .run()
        .policy()
        .concurrency()
        .parallel_attempts()
        .get()
        != 1)
        .then(|| "parallel execution is not implemented".to_owned());
    for job in plan.jobs() {
        if cancellation.is_cancelled() || error.is_some() {
            break;
        }
        let result = session::Session::new(&mut run, plan, job, cancellation)
            .and_then(|mut session| stages::execute(&mut session));
        if let Err(failure) = result {
            error = Some(failure.to_string());
            break;
        }
    }
    let cancelled = cancellation.is_cancelled();
    if error.is_some() || cancelled {
        run.store(
            &crate::ArtifactRequest {
                path: "logs/execution-failure.json".into(),
                name: crate::NonEmptyString::new("execution failure")?,
                kind: crate::ArtifactKind::Log,
                media_type: "application/json".into(),
                attempt_id: None,
            },
            &serde_json::to_vec_pretty(&serde_json::json!({"cancelled":cancelled,"error":error}))?,
        )?;
    }
    let outcome = if error.is_some() || cancelled {
        RunOutcome::Failed
    } else {
        RunOutcome::Completed
    };
    let record = run.finish(outcome)?;
    Ok(ExecutionResult {
        directory,
        record,
        cancelled,
        error,
    })
}
