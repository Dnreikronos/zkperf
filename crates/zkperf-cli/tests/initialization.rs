mod support;

use std::fs;
use std::process::Stdio;

use support::{Fixture, assert_status, command};

#[test]
fn init_is_non_interactive_and_its_manifest_validates_from_another_directory() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("nested/my starter");
    let output = command()
        .arg("init")
        .arg(&destination)
        .stdin(Stdio::null())
        .env("ZKPERF_MANIFEST", "missing")
        .env("ZKPERF_RUNS", "invalid")
        .env("ZKPERF_FORMAT", "invalid")
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Created starter manifest:"));
    assert!(stdout.contains("Next: zkperf validate --manifest '"));
    let manifest = destination.join("zkperf.toml");
    assert!(stdout.contains(&manifest.display().to_string()));
    for args in [vec!["validate"], vec!["run", "--print-config"]] {
        let output = command()
            .args(args)
            .arg("--manifest")
            .arg(&manifest)
            .output()
            .unwrap();
        assert_status(&output, 0);
    }
}

#[test]
fn default_destination_conflicts_and_force_have_the_documented_behavior() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("starter");
    fs::create_dir(&destination).unwrap();
    let init = || {
        let mut cmd = command();
        cmd.current_dir(&destination)
            .arg("init")
            .stdin(Stdio::null());
        cmd
    };
    assert_status(&init().output().unwrap(), 0);
    let manifest = destination.join("zkperf.toml");
    let original = fs::read(&manifest).unwrap();
    fs::write(&manifest, "user manifest").unwrap();
    fs::write(destination.join("notes.txt"), "keep me").unwrap();
    let output = init().arg("--quiet").output().unwrap();
    assert_status(&output, 4);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[io]"));
    assert!(stderr.contains("--force"));
    assert_eq!(fs::read(&manifest).unwrap(), b"user manifest");
    assert_status(&init().arg("--force").output().unwrap(), 0);
    assert_eq!(fs::read(manifest).unwrap(), original);
    assert_eq!(fs::read(destination.join("notes.txt")).unwrap(), b"keep me");
}

#[test]
fn an_invalid_destination_returns_a_path_specific_io_error() {
    let fixture = Fixture::new();
    let output = command()
        .arg("init")
        .arg(fixture.manifest())
        .output()
        .unwrap();
    assert_status(&output, 4);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[io]"));
    assert!(stderr.contains("zkperf.toml"));
}

#[cfg(unix)]
#[test]
fn next_action_quotes_shell_metacharacters_and_can_be_executed() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("a '$dollar `backtick` directory");
    let output = command().arg("init").arg(&destination).output().unwrap();
    assert_status(&output, 0);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let next = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Next: "))
        .unwrap();
    let args = next.strip_prefix("zkperf ").unwrap();
    // Supply the executable separately so this exercises the printed argument
    // quoting without relying on a globally installed zkperf binary.
    let output = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("exec \"$1\" {args}"),
            "sh",
            env!("CARGO_BIN_EXE_zkperf"),
        ])
        .env_clear()
        .output()
        .unwrap();
    assert_status(&output, 0);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Manifest is valid:")
    );
}
