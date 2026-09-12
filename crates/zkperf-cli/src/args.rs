use std::num::NonZeroU64;
use std::path::PathBuf;

use clap::builder::{OsStringValueParser, TypedValueParser};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use zkperf_core::OutputFormat;

#[derive(Debug, Parser)]
#[command(
    name = "zkperf",
    version,
    about = "Reproducible zero-knowledge proof benchmarks"
)]
#[command(
    after_help = "Configuration: flags > environment > manifest > defaults. See docs/cli.md."
)]
pub struct Cli {
    #[command(flatten)]
    pub logging: LoggingArgs,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn parse_args() -> Result<Self, clap::Error> {
        let cli = Self::try_parse()?;
        // Clap checks conflicts before propagating globals across subcommands.
        let logging_options = [
            cli.logging.quiet,
            cli.logging.verbose,
            cli.logging.log_level.is_some(),
        ];
        if logging_options
            .into_iter()
            .filter(|selected| *selected)
            .count()
            > 1
        {
            return Err(Self::command().error(
                clap::error::ErrorKind::ArgumentConflict,
                "--quiet, --verbose, and --log-level cannot be used together",
            ));
        }
        Ok(cli)
    }
}

#[derive(Debug, Args)]
pub struct LoggingArgs {
    /// Suppress progress logs; errors remain visible
    #[arg(short, long, global = true, conflicts_with_all = ["verbose", "log_level"])]
    pub quiet: bool,
    /// Enable debug logging
    #[arg(short, long, global = true, conflicts_with = "log_level")]
    pub verbose: bool,
    /// Logging verbosity [env: `ZKPERF_LOG`] [default: info]
    #[arg(long, global = true, value_enum)]
    pub log_level: Option<LogLevel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a starter benchmark directory without prompting
    #[command(
        after_help = "Example: zkperf init ./benchmarks\nExisting template files require --force to replace."
    )]
    Init(InitArgs),
    /// Validate a manifest, fixture paths, and effective benchmark settings
    #[command(after_help = "Example: zkperf validate --manifest suite/zkperf.toml --print-config")]
    Validate(BenchmarkArgs),
    /// Run benchmarks (execution service pending); inspect with --print-config
    #[command(
        after_help = "Example: zkperf run --runs 10 --print-config\nExecution is tracked in issues #9–15."
    )]
    Run(BenchmarkArgs),
    /// Render a stored benchmark report (reporting service pending)
    #[command(
        after_help = "Example: zkperf report runs/report.json --format html --output report.html"
    )]
    Report(ReportArgs),
    /// Compare baseline and candidate reports (comparison service pending)
    #[command(after_help = "Example: zkperf compare baseline.json candidate.json --format json")]
    Compare(CompareArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Directory to initialize
    #[arg(default_value = ".", value_parser = non_empty_path())]
    pub directory: PathBuf,
    /// Permit replacement of existing template files
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct BenchmarkArgs {
    /// Manifest file [env: `ZKPERF_MANIFEST`] [default: zkperf.toml]
    #[arg(short, long, value_name = "FILE", value_parser = non_empty_path())]
    pub manifest: Option<PathBuf>,
    /// Warmup repetitions; zero is allowed [env: `ZKPERF_WARMUPS`]
    #[arg(long, value_name = "N")]
    pub warmups: Option<u64>,
    /// Measured repetitions; must be positive [env: `ZKPERF_RUNS`]
    #[arg(long, value_name = "N")]
    pub runs: Option<NonZeroU64>,
    /// Output directory relative to the manifest [env: `ZKPERF_OUTPUT_DIR`]
    #[arg(long, value_name = "DIRECTORY", value_parser = non_empty_path())]
    pub output_dir: Option<PathBuf>,
    /// Replace output formats (comma-separated) [env: `ZKPERF_FORMAT`]
    #[arg(long, value_enum, value_delimiter = ',')]
    pub format: Vec<Format>,
    /// Print the effective, redacted JSON configuration without executing
    #[arg(long)]
    pub print_config: bool,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Stored `BenchmarkReport` JSON file
    #[arg(value_parser = non_empty_path())]
    pub input: PathBuf,
    /// Render format [env: `ZKPERF_FORMAT`] [default: terminal]
    #[arg(long, value_enum)]
    pub format: Option<Format>,
    /// Write to this file instead of stdout
    #[arg(short, long, value_name = "FILE", value_parser = non_empty_path())]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CompareArgs {
    /// Baseline `BenchmarkReport` JSON file
    #[arg(value_parser = non_empty_path())]
    pub baseline: PathBuf,
    /// Candidate `BenchmarkReport` JSON file
    #[arg(value_parser = non_empty_path())]
    pub candidate: PathBuf,
    /// Comparison format [env: `ZKPERF_FORMAT`] [default: terminal]
    #[arg(long, value_enum)]
    pub format: Option<ComparisonFormat>,
    /// Write to this file instead of stdout
    #[arg(short, long, value_name = "FILE", value_parser = non_empty_path())]
    pub output: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Terminal,
    Json,
    Html,
    Csv,
}

impl From<Format> for OutputFormat {
    fn from(value: Format) -> Self {
        match value {
            Format::Terminal => Self::Terminal,
            Format::Json => Self::Json,
            Format::Html => Self::Html,
            Format::Csv => Self::Csv,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ComparisonFormat {
    Terminal,
    Json,
}

fn non_empty_path() -> impl TypedValueParser<Value = PathBuf> {
    OsStringValueParser::new().try_map(|value| {
        if value.is_empty() {
            Err("path must not be empty".to_owned())
        } else {
            Ok(PathBuf::from(value))
        }
    })
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_consistent() {
        super::Cli::command().debug_assert();
    }
}
