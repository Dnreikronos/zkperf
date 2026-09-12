use std::io::Write;

use crate::args::{Command, ComparisonFormat, Format, LogLevel};
use crate::config::{self, Environment};
use crate::diagnostics::{Diagnostic, Logger};

pub fn execute(
    command: Command,
    env: Environment<'_>,
    logger: &mut Logger<impl Write>,
) -> Result<String, Diagnostic> {
    match command {
        Command::Validate(args) => benchmark(args, false, env, logger),
        Command::Run(args) => benchmark(args, true, env, logger),
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
    if execute {
        return Err(Diagnostic::unavailable("run execution", "issues #9–15"));
    }
    Ok(format!(
        "Manifest is valid: {}",
        manifest.manifest_path().display()
    ))
}
