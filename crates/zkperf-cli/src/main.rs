mod args;
mod commands;
mod config;
mod diagnostics;

use std::io;
use std::process::ExitCode;

use args::Cli;
use diagnostics::{Diagnostic, Logger};

fn main() -> ExitCode {
    let cli = match Cli::parse_args() {
        Ok(cli) => cli,
        Err(error) => {
            let status = if error.use_stderr() { 2 } else { 0 };
            return ExitCode::from(if error.print().is_ok() { status } else { 4 });
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = error.emit(&mut io::stderr().lock());
            ExitCode::from(error.exit_code)
        }
    }
}

fn run(cli: Cli) -> Result<(), Diagnostic> {
    let env = |key: &str| std::env::var_os(key);
    let level = config::log_level(&cli.logging, &env)?;
    let mut logger = Logger::new(level, io::stderr().lock());
    let output = commands::execute(cli.command, &env, &mut logger)?;
    diagnostics::write_output(&mut io::stdout().lock(), &output)
}
