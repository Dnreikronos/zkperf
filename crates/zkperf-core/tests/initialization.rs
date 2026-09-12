use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use zkperf_core::{BenchmarkManifest, initialize};

const FILES: &[&str] = &[
    "zkperf.toml",
    "benchmarks/sha256.md",
    "benchmarks/input.bin",
    "benchmarks/output.bin",
    "benchmarks/mock.zkperf-adapter.json",
];

struct Suite(PathBuf);

impl Suite {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zkperf-init-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn starter_matches_the_current_schema_and_hashes_the_documented_input() {
    let suite = Suite::new();
    let root = suite.0.join("nested/starter");
    initialize(&root, false).unwrap();
    let manifest = BenchmarkManifest::load(root.join("zkperf.toml")).unwrap();
    assert_eq!(manifest.workloads().len(), 1);
    assert_eq!(manifest.engines().len(), 1);
    let input = fs::read(root.join("benchmarks/input.bin")).unwrap();
    assert_eq!(input, b"abc");
    assert_eq!(
        fs::read(root.join("benchmarks/output.bin")).unwrap(),
        Sha256::digest(&input).to_vec()
    );
    assert!(!root.join("runs").exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
    assert_eq!(fs::read_dir(root.join("benchmarks")).unwrap().count(), 4);
}

#[test]
fn bytes_are_deterministic_and_force_preserves_unrelated_files() {
    let first = Suite::new();
    let second = Suite::new();
    initialize(&first.0, false).unwrap();
    initialize(&second.0, false).unwrap();
    for file in FILES {
        assert_eq!(
            fs::read(first.0.join(file)).unwrap(),
            fs::read(second.0.join(file)).unwrap()
        );
        fs::write(second.0.join(file), "user edit").unwrap();
    }
    fs::write(second.0.join("notes.txt"), "keep me").unwrap();
    fs::write(second.0.join("benchmarks/custom.bin"), "custom").unwrap();
    initialize(&second.0, true).unwrap();
    for file in FILES {
        assert_eq!(
            fs::read(first.0.join(file)).unwrap(),
            fs::read(second.0.join(file)).unwrap()
        );
    }
    assert_eq!(fs::read(second.0.join("notes.txt")).unwrap(), b"keep me");
    assert_eq!(
        fs::read(second.0.join("benchmarks/custom.bin")).unwrap(),
        b"custom"
    );
    BenchmarkManifest::load(second.0.join("zkperf.toml")).unwrap();
}

#[test]
fn every_file_conflict_is_detected_before_any_template_is_written() {
    for conflict in FILES {
        let suite = Suite::new();
        let path = suite.0.join(conflict);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "keep me").unwrap();
        let error = initialize(&suite.0, false).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("--force"));
        assert!(error.to_string().contains(conflict));
        assert_eq!(fs::read(&path).unwrap(), b"keep me");
        for other in FILES.iter().filter(|other| *other != conflict) {
            assert!(!suite.0.join(other).exists());
        }
    }
}

#[test]
fn force_does_not_replace_directories_and_preflight_preserves_existing_files() {
    let suite = Suite::new();
    initialize(&suite.0, false).unwrap();
    let manifest = suite.0.join("zkperf.toml");
    fs::write(&manifest, "user manifest").unwrap();
    let output = suite.0.join("benchmarks/output.bin");
    fs::remove_file(&output).unwrap();
    fs::create_dir(&output).unwrap();
    assert!(initialize(&suite.0, true).is_err());
    assert_eq!(fs::read(manifest).unwrap(), b"user manifest");
    assert!(output.is_dir());
}

#[test]
fn files_cannot_be_used_as_the_root_or_benchmark_directory() {
    for relative in ["destination", "destination/benchmarks"] {
        let suite = Suite::new();
        let path = suite.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "keep me").unwrap();
        for force in [false, true] {
            assert!(initialize(suite.0.join("destination"), force).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"keep me");
        }
    }
}

#[cfg(unix)]
#[test]
fn symlinks_in_template_paths_are_rejected_including_dangling_links() {
    use std::os::unix::fs::symlink;

    for relative in FILES.iter().copied().chain(["benchmarks"]) {
        for dangling in [false, true] {
            let suite = Suite::new();
            let root = suite.0.join("starter");
            let outside = suite.0.join("outside");
            if !dangling {
                if relative == "benchmarks" {
                    fs::create_dir(&outside).unwrap();
                } else {
                    fs::write(&outside, "keep me").unwrap();
                }
            }
            let link = root.join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside, &link).unwrap();
            for force in [false, true] {
                assert!(initialize(&root, force).is_err());
                assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
                if dangling {
                    assert!(!outside.exists());
                } else if relative == "benchmarks" {
                    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
                } else {
                    assert_eq!(fs::read(&outside).unwrap(), b"keep me");
                }
            }
        }
    }
}

#[test]
fn force_replaces_hard_links_without_modifying_the_other_name() {
    let suite = Suite::new();
    let outside = suite.0.join("original");
    fs::write(&outside, "keep me").unwrap();
    fs::hard_link(&outside, suite.0.join("zkperf.toml")).unwrap();
    initialize(&suite.0, true).unwrap();
    assert_eq!(fs::read(outside).unwrap(), b"keep me");
    BenchmarkManifest::load(suite.0.join("zkperf.toml")).unwrap();
}
