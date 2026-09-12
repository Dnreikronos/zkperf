use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zkperf-core/tests/fixtures/manifest")
        .canonicalize()
        .unwrap()
}

pub fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zkperf"));
    for key in [
        "ZKPERF_MANIFEST",
        "ZKPERF_WARMUPS",
        "ZKPERF_RUNS",
        "ZKPERF_OUTPUT_DIR",
        "ZKPERF_FORMAT",
        "ZKPERF_LOG",
    ] {
        command.env_remove(key);
    }
    command.current_dir(fixture_root());
    command
}

pub fn assert_status(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub struct Fixture(pub PathBuf);

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for file in [
            "zkperf.toml",
            "workload.md",
            "input.bin",
            "output.bin",
            "mock.zkperf-adapter.json",
        ] {
            fs::copy(fixture_root().join(file), root.join(file)).unwrap();
        }
        Self(root)
    }

    pub fn manifest(&self) -> PathBuf {
        self.0.join("zkperf.toml")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
