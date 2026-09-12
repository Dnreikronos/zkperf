use std::io::Write;

use zkperf_core::{BenchmarkManifest, BenchmarkPlan, CancellationToken};

use crate::args::LogLevel;
use crate::diagnostics::{Diagnostic, Logger};

pub(super) fn execute(
    manifest: BenchmarkManifest,
    logger: &mut Logger<impl Write>,
) -> Result<String, Diagnostic> {
    let plan = BenchmarkPlan::build(manifest)
        .map_err(|error| Diagnostic::configuration(error.to_string()))?;
    let cancellation = CancellationToken::default();
    let signal = cancellation.clone();
    ctrlc::set_handler(move || signal.cancel())
        .map_err(|error| Diagnostic::runtime(error.to_string(), false))?;
    logger.log(
        LogLevel::Info,
        &format!("Executing {} planned jobs", plan.jobs().len()),
    )?;
    let result = zkperf_core::execute_plan(&plan, &cancellation)
        .map_err(|error| Diagnostic::runtime(error.to_string(), cancellation.is_cancelled()))?;
    if result.cancelled || result.error.is_some() {
        return Err(Diagnostic::runtime(
            format!(
                "{}; run evidence: {}",
                result.error.as_deref().unwrap_or("run cancelled"),
                result.directory.display()
            ),
            result.cancelled,
        ));
    }
    Ok(format!(
        "Completed {} jobs. Run evidence: {}",
        plan.jobs().len(),
        result.directory.display()
    ))
}
