mod plan;
mod run;

use std::io::Write;

use crate::args::{Command, ComparisonFormat, Format, LogLevel, PlanFormat};
use crate::config::{self, Environment};
use crate::diagnostics::{Diagnostic, Logger};

pub fn execute(
    command: Command,
    env: Environment<'_>,
    logger: &mut Logger<impl Write>,
) -> Result<String, Diagnostic> {
    match command {
        Command::Validate(args) => benchmark(args, false, None, env, logger),
        Command::Run(args) => benchmark(
            args.benchmark,
            true,
            args.dry_run
                .then_some(args.plan_format.unwrap_or(PlanFormat::Json)),
            env,
            logger,
        ),
        Command::Init(args) => {
            zkperf_core::initialize(&args.directory, args.force)?;
            let manifest = args.directory.join("zkperf.toml");
            let path = manifest.to_string_lossy();
            // Single quotes protect spaces and shell metacharacters in POSIX
            // shells and PowerShell; their embedded-quote escapes differ.
            let quoted = if cfg!(windows) {
                path.replace('\'', "''")
            } else {
                path.replace('\'', "'\"'\"'")
            };
            Ok(format!(
                "Created starter manifest: {}\nNext: zkperf validate --manifest '{quoted}'",
                manifest.display()
            ))
        }
        Command::Report(args) => {
            config::format(args.format, env, Format::Terminal)?;
            Err(Diagnostic::unavailable("report", "issues #16, #20 and #21"))
        }
        Command::Compare(args) => {
            config::format(args.format, env, ComparisonFormat::Terminal)?;
            Err(Diagnostic::unavailable("compare", "issue #22"))
        }
    }
}

fn benchmark(
    args: crate::args::BenchmarkArgs,
    execute: bool,
    plan_format: Option<PlanFormat>,
    env: Environment<'_>,
    logger: &mut Logger<impl Write>,
) -> Result<String, Diagnostic> {
    let print_config = args.print_config;
    logger.log(LogLevel::Info, "Validating benchmark configuration")?;
    let manifest = config::benchmark(args, env)?;
    logger.log(
        LogLevel::Debug,
        &format!("Loaded {}", manifest.manifest_path().display()),
    )?;
    if print_config {
        return manifest
            .normalized_debug()
            .map_err(|error| Diagnostic::configuration(error.to_string()));
    }
    if let Some(format) = plan_format {
        let plan = zkperf_core::BenchmarkPlan::build(manifest)
            .map_err(|error| Diagnostic::configuration(error.to_string()))?;
        return plan::render(&plan, format);
    }
    if execute {
        return run::execute(manifest, logger);
    }
    Ok(format!(
        "Manifest is valid: {}",
        manifest.manifest_path().display()
    ))
}
