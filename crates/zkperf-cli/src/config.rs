use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;

use clap::ValueEnum;
use zkperf_core::{BenchmarkManifest, ManifestOverrides};

use crate::args::{BenchmarkArgs, Format, LogLevel, LoggingArgs};
use crate::diagnostics::Diagnostic;

pub type Environment<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub fn log_level(args: &LoggingArgs, env: Environment<'_>) -> Result<LogLevel, Diagnostic> {
    let flag = if args.quiet {
        Some(LogLevel::Off)
    } else if args.verbose {
        Some(LogLevel::Debug)
    } else {
        args.log_level
    };
    Ok(select(flag, "ZKPERF_LOG", env, enumeration)?.unwrap_or(LogLevel::Info))
}

pub fn benchmark(
    args: BenchmarkArgs,
    env: Environment<'_>,
) -> Result<BenchmarkManifest, Diagnostic> {
    let manifest_path = select(args.manifest, "ZKPERF_MANIFEST", env, path)?
        .unwrap_or_else(|| PathBuf::from("zkperf.toml"));
    let manifest = BenchmarkManifest::load(manifest_path)
        .map_err(|error| Diagnostic::configuration(error.to_string()))?;
    let formats = (!args.format.is_empty()).then_some(args.format);
    let overrides = ManifestOverrides {
        warmups: select(args.warmups, "ZKPERF_WARMUPS", env, number)?,
        runs: select(args.runs, "ZKPERF_RUNS", env, number)?,
        output_directory: select(args.output_dir, "ZKPERF_OUTPUT_DIR", env, path)?,
        output_formats: select(formats, "ZKPERF_FORMAT", env, format_list)?
            .map(|formats| formats.into_iter().map(Into::into).collect()),
    };
    manifest
        .with_overrides(overrides)
        .map_err(|error| Diagnostic::configuration(error.to_string()))
}

pub fn format<T: ValueEnum>(
    flag: Option<T>,
    env: Environment<'_>,
    default: T,
) -> Result<T, Diagnostic> {
    Ok(select(flag, "ZKPERF_FORMAT", env, enumeration)?.unwrap_or(default))
}

fn select<T>(
    flag: Option<T>,
    key: &str,
    env: Environment<'_>,
    parse: impl FnOnce(OsString) -> Option<T>,
) -> Result<Option<T>, Diagnostic> {
    if flag.is_some() {
        return Ok(flag);
    }
    env(key)
        .map(|value| {
            parse(value).ok_or_else(|| {
                Diagnostic::configuration(format!(
                    "{key}: invalid value; see the command's --help for accepted values"
                ))
            })
        })
        .transpose()
}

fn path(value: OsString) -> Option<PathBuf> {
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn number<T: FromStr>(value: OsString) -> Option<T> {
    value.into_string().ok()?.parse().ok()
}

fn enumeration<T: ValueEnum>(value: OsString) -> Option<T> {
    T::from_str(&value.into_string().ok()?, false).ok()
}

fn format_list(value: OsString) -> Option<Vec<Format>> {
    value
        .into_string()
        .ok()?
        .split(',')
        .map(|value| Format::from_str(value, false).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Format, format};

    #[test]
    fn report_format_uses_flags_then_environment_then_default() {
        let env = |key: &str| (key == "ZKPERF_FORMAT").then(|| "csv".into());
        assert!(matches!(
            format(None, &|_| None, Format::Terminal).unwrap(),
            Format::Terminal
        ));
        assert!(matches!(
            format(None, &env, Format::Terminal).unwrap(),
            Format::Csv
        ));
        assert!(matches!(
            format(Some(Format::Json), &env, Format::Terminal).unwrap(),
            Format::Json
        ));
    }
}
