#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zkperf_core::{BenchmarkManifest, ManifestError, ManifestOverrides};

const SOURCE: &str = include_str!("fixtures/manifest/zkperf.toml");

struct Suite(PathBuf);

impl Suite {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-output-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("suite")).unwrap();
        fs::create_dir(root.join("suite-other")).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/manifest");
        for file in [
            "workload.md",
            "input.bin",
            "output.bin",
            "mock.zkperf-adapter.json",
        ] {
            fs::copy(fixtures.join(file), root.join("suite").join(file)).unwrap();
        }
        Self(root)
    }

    fn directory(&self) -> PathBuf {
        self.0.join("suite")
    }

    fn load(
        &self,
        directory: &str,
        via_override: bool,
    ) -> Result<BenchmarkManifest, ManifestError> {
        let path = self.directory().join("zkperf.toml");
        if via_override {
            fs::write(&path, SOURCE).unwrap();
            BenchmarkManifest::load(&path)?.with_overrides(ManifestOverrides {
                output_directory: Some(directory.into()),
                ..ManifestOverrides::default()
            })
        } else {
            fs::write(
                &path,
                SOURCE.replace(
                    "directory = \"results\"",
                    &format!("directory = \"{directory}\""),
                ),
            )
            .unwrap();
            BenchmarkManifest::load(&path)
        }
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn output_symlinks_cannot_escape_for_manifests_or_overrides() {
    let suite = Suite::new();
    let outside = suite.0.join("suite-other");
    symlink(&outside, suite.directory().join("escape")).unwrap();
    for directory in ["escape", "escape/missing/nested"] {
        for via_override in [false, true] {
            let error = suite
                .load(directory, via_override)
                .expect_err("output must stay inside suite");
            assert_eq!(error.field_path(), "outputs.directory");
            assert!(error.message().contains("within the manifest directory"));
        }
    }
    assert!(!outside.join("missing").exists());
}

#[test]
fn internal_symlinks_resolve_existing_and_missing_output_directories() {
    let suite = Suite::new();
    let target = suite.directory().join("actual");
    fs::create_dir(&target).unwrap();
    symlink("actual", suite.directory().join("alias")).unwrap();
    for (directory, suffix) in [("alias", ""), ("alias/missing/nested", "missing/nested")] {
        for via_override in [false, true] {
            let manifest = suite.load(directory, via_override).unwrap();
            assert_eq!(
                manifest.outputs().directory(),
                target.canonicalize().unwrap().join(suffix)
            );
        }
    }
    assert!(!target.join("missing").exists());
}

#[test]
fn dangling_output_symlinks_are_not_treated_as_missing_directories() {
    let suite = Suite::new();
    symlink(suite.0.join("absent"), suite.directory().join("dangling")).unwrap();
    for directory in ["dangling", "dangling/nested"] {
        for via_override in [false, true] {
            let error = suite
                .load(directory, via_override)
                .expect_err("unresolved symlink must be rejected");
            assert_eq!(error.field_path(), "outputs.directory");
        }
    }
}
