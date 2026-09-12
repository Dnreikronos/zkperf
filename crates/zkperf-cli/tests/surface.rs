mod support;

use std::fs;
use std::path::Path;

use support::{Fixture, assert_status, command};

#[test]
fn every_command_has_useful_help_without_loading_configuration() {
    let executable = Path::new(env!("CARGO_BIN_EXE_zkperf"))
        .file_name()
        .expect("Cargo binary path should have a filename")
        .to_string_lossy();
    for name in ["init", "validate", "run", "report", "compare"] {
        let output = command()
            .args([name, "--help"])
            .env("ZKPERF_MANIFEST", "missing.toml")
            .env("ZKPERF_LOG", "invalid")
            .output()
            .unwrap();
        assert_status(&output, 0);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(&format!("Usage: {executable} {name}")),
            "{text}"
        );
        assert!(text.contains("Example:"), "{text}");
        assert!(text.contains("--log-level"), "{text}");
        assert!(output.stderr.is_empty());
    }
    for flag in ["--help", "--version"] {
        let output = command().arg(flag).output().unwrap();
        assert_status(&output, 0);
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn invalid_and_conflicting_arguments_are_usage_errors() {
    let cases: &[&[&str]] = &[
        &[],
        &["unknown"],
        &["report"],
        &["compare", "baseline.json"],
        &["validate", "--runs", "0"],
        &["validate", "--runs", "-1"],
        &["validate", "--warmups", "-1"],
        &["validate", "--runs", "18446744073709551616"],
        &["validate", "--format", "xml"],
        &["report", "x.json", "--format", "json,csv"],
        &["compare", "a", "b", "--format", "html"],
        &["validate", "--manifest", ""],
        &["validate", "--output-dir", ""],
        &["report", ""],
        &["report", "x.json", "--output", ""],
        &["--quiet", "--verbose", "validate"],
        &["--quiet", "validate", "--verbose"],
        &["validate", "--quiet", "--log-level", "debug"],
        &["--verbose", "validate", "--log-level", "info"],
        &["validate", "--log-level", "invalid"],
        &["init", "--manifest", "x"],
        &["validate", "--runs", "1", "--runs", "2"],
    ];
    for args in cases {
        let output = command().args(*args).output().unwrap();
        assert_status(&output, 2);
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn global_logging_flags_work_on_either_side_of_commands() {
    for args in [["--quiet", "validate"], ["validate", "--quiet"]] {
        let output = command()
            .args(args)
            .env("ZKPERF_LOG", "malformed")
            .output()
            .unwrap();
        assert_status(&output, 0);
        assert!(output.stderr.is_empty());
    }
    let default = command().arg("validate").output().unwrap();
    assert_status(&default, 0);
    let stderr = String::from_utf8(default.stderr).unwrap();
    assert!(stderr.contains("Info"));
    assert!(!stderr.contains("Debug"));
    for args in [["--verbose", "validate"], ["validate", "--verbose"]] {
        let output = command()
            .args(args)
            .env("ZKPERF_LOG", "off")
            .output()
            .unwrap();
        assert_status(&output, 0);
        assert!(String::from_utf8(output.stderr).unwrap().contains("Debug"));
    }
    let output = command()
        .arg("validate")
        .env("ZKPERF_LOG", "debug")
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(String::from_utf8(output.stderr).unwrap().contains("Debug"));
    let output = command()
        .args(["validate", "--log-level", "off"])
        .env("ZKPERF_LOG", "debug")
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(output.stderr.is_empty());
}

#[test]
fn pending_services_fail_explicitly_without_writing_files() {
    let fixture = Fixture::new();
    let source = fs::read(fixture.manifest()).unwrap();
    let initial_count = fs::read_dir(&fixture.0).unwrap().count();
    for args in [
        vec!["report", "missing.json", "--output", "report.html"],
        vec!["compare", "a.json", "b.json", "--output", "comparison.json"],
    ] {
        let output = command()
            .args(args)
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert_status(&output, 5);
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("error[unavailable]"));
        assert!(stderr.contains("not implemented"));
    }
    assert_eq!(fs::read(fixture.manifest()).unwrap(), source);
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), initial_count);
}

#[test]
fn manifest_diagnostics_are_actionable_even_in_quiet_mode() {
    let fixture = Fixture::new();
    fs::write(fixture.manifest(), "manifest_version = '1.0.0'\n").unwrap();
    let output = command()
        .args(["--quiet", "validate"])
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    assert_status(&output, 3);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[configuration]"));
    assert!(stderr.contains("run"));
}

#[test]
fn report_and_compare_validate_only_their_relevant_environment() {
    for args in [vec!["report", "a"], vec!["compare", "a", "b"]] {
        let output = command()
            .args(args)
            .env("ZKPERF_MANIFEST", "missing")
            .env("ZKPERF_RUNS", "invalid")
            .output()
            .unwrap();
        assert_status(&output, 5);
    }
    let output = command()
        .args(["compare", "a", "b"])
        .env("ZKPERF_FORMAT", "html")
        .output()
        .unwrap();
    assert_status(&output, 3);
    let output = command()
        .args(["compare", "a", "b", "--format", "json"])
        .env("ZKPERF_FORMAT", "html")
        .output()
        .unwrap();
    assert_status(&output, 5);
}

#[cfg(unix)]
#[test]
fn diagnostic_write_failures_override_the_original_error_status() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    for (args, original_status) in [
        (vec!["--quiet", "validate", "--manifest", "missing.toml"], 3),
        (vec!["--quiet", "report", "missing.json"], 5),
    ] {
        let normal = command().args(&args).output().unwrap();
        assert_status(&normal, original_status);
        assert!(!normal.stderr.is_empty());

        let (writer, reader) = UnixStream::pair().unwrap();
        drop(reader);
        let output = command()
            .args(&args)
            .stderr(Stdio::from(OwnedFd::from(writer)))
            .output()
            .unwrap();
        assert_status(&output, 4);
        assert!(output.stdout.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn a_closed_stdout_returns_io_status_without_panicking() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    let (writer, reader) = UnixStream::pair().unwrap();
    drop(reader);
    let output = command()
        .args(["validate", "--quiet"])
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .output()
        .unwrap();
    assert_status(&output, 4);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[io]"));
    assert!(!stderr.contains("panicked"));
}
