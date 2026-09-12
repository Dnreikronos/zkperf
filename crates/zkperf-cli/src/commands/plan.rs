use std::fmt::Write as _;

use zkperf_core::{BenchmarkPlan, Slug};

use crate::args::PlanFormat;
use crate::diagnostics::Diagnostic;

pub(super) fn render(plan: &BenchmarkPlan, format: PlanFormat) -> Result<String, Diagnostic> {
    match format {
        PlanFormat::Json => plan
            .normalized_debug()
            .map_err(|error| Diagnostic::configuration(error.to_string())),
        PlanFormat::Table => {
            table(plan).map_err(|error| Diagnostic::configuration(error.to_string()))
        }
    }
}

fn table(plan: &BenchmarkPlan) -> Result<String, std::fmt::Error> {
    let manifest = plan.manifest();
    let warmups = plan.jobs().iter().filter(|job| job.warmup()).count();
    let mut output = format!(
        "Plan: {}\nManifest: {}\nOrdering: {} (seed {})\nJobs: {} warm-up, {} measured\nJob IDs: {}:<position>\n\n",
        plan.id(),
        manifest
            .manifest_path()
            .display()
            .to_string()
            .escape_debug(),
        manifest.run().policy().ordering_algorithm().as_str(),
        manifest.run().policy().seed(),
        warmups,
        plan.jobs().len() - warmups,
        plan.id(),
    );
    let mut rows = vec![vec![
        "POSITION".to_owned(),
        "KIND".to_owned(),
        "REPETITION".to_owned(),
        "ENGINE".to_owned(),
        "WORKLOAD".to_owned(),
        "INPUT".to_owned(),
        "PROOF MODE".to_owned(),
    ]];
    for job in plan.jobs() {
        rows.push(vec![
            job.position().to_string(),
            if job.warmup() { "warm-up" } else { "measured" }.to_owned(),
            job.repetition().to_string(),
            job.engine_id().as_str().to_owned(),
            job.workload_id().as_str().to_owned(),
            job.input_id().as_str().to_owned(),
            job.proof_mode().map_or("-", Slug::as_str).to_owned(),
        ]);
    }
    let widths: Vec<_> = (0..rows[0].len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    for row in rows {
        for (column, value) in row.iter().enumerate() {
            if column > 0 {
                output.push_str("  ");
            }
            write!(output, "{value:width$}", width = widths[column])?;
        }
        output.truncate(output.trim_end().len());
        output.push('\n');
    }
    Ok(output.trim_end().to_owned())
}
