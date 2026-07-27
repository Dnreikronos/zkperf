use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zkperf_core::{BenchmarkManifest, ManifestPhase, OutputFormat, SchemaVersion};

const VALID_MANIFEST: &str = include_str!("fixtures/manifest/zkperf.toml");
const SUPPORT_FILES: &[&str] = &[
    "workload.md",
    "input.bin",
    "output.bin",
    "mock.zkperf-adapter.json",
];

#[test]
fn representative_manifest_loads_deterministically() {
    let manifest_path = fixture_root().join("zkperf.toml");
    let first = BenchmarkManifest::load(&manifest_path).expect("fixture manifest should load");
    let second = BenchmarkManifest::load(&manifest_path).expect("fixture manifest should reload");

    assert_eq!(first, second);
    assert_eq!(first.manifest_version(), SchemaVersion::V1_0_0);
    assert_eq!(first.run().warmups(), 1);
    assert_eq!(first.run().runs().get(), 3);
    assert_eq!(
        first.outputs().formats(),
        &[OutputFormat::Json, OutputFormat::Terminal]
    );
    assert_eq!(
        first.workloads()[0].phases(),
        &[
            ManifestPhase::Execution,
            ManifestPhase::Proving,
            ManifestPhase::Verification,
        ]
    );
    assert!(first.manifest_path().is_absolute());
    assert!(
        first.workloads()[0].inputs()[0]
            .fixture()
            .path()
            .is_absolute()
    );
    assert_eq!(
        first.workloads()[0].inputs()[0]
            .fixture()
            .sha256()
            .expect("input fixture should be hashable")
            .value(),
        "1c8eb85217cc5b8ddd7e3c488a20efce3b958d59199f3b51b503c57c7b7c3828"
    );
}

#[test]
fn normalized_debug_output_matches_snapshot() {
    let root = fixture_root()
        .canonicalize()
        .expect("fixture root should resolve");
    let manifest =
        BenchmarkManifest::load(root.join("zkperf.toml")).expect("fixture manifest should load");
    let normalized = manifest
        .normalized_debug()
        .expect("validated manifest should serialize")
        .replace(&root.to_string_lossy().to_string(), "$FIXTURE_ROOT")
        .replace('\\', "/");

    assert_eq!(
        normalized,
        include_str!("snapshots/manifest_normalized.json").trim_end()
    );
}

#[test]
fn validation_errors_identify_exact_field_paths() {
    let cases = [
        (VALID_MANIFEST.replace("runs = 3", "runs = 0"), "run.runs"),
        (
            VALID_MANIFEST.replacen(
                "termination_grace_ms = 1_000",
                "termination_grace_ms = 30_001",
                1,
            ),
            "run.timeouts[0].termination_grace_ms",
        ),
        (
            VALID_MANIFEST.replacen("phase = \"verification\"", "phase = \"proving\"", 1),
            "run.timeouts[2].phase",
        ),
        (
            VALID_MANIFEST.replace(
                "formats = [\"json\", \"terminal\"]",
                "formats = [\"json\", \"json\"]",
            ),
            "outputs.formats[1]",
        ),
        (
            VALID_MANIFEST.replace("directory = \"results\"", "directory = \"\""),
            "outputs.directory",
        ),
        (
            VALID_MANIFEST.replace(
                "phases = [\"execution\", \"proving\", \"verification\"]",
                "phases = [\"execution\", \"execution\", \"verification\"]",
            ),
            "workloads[0].phases[1]",
        ),
        (
            VALID_MANIFEST.replace("fixture = \"input.bin\"", "fixture = \"missing.bin\""),
            "workloads[0].inputs[0].fixture",
        ),
        (
            VALID_MANIFEST.replace("proof_modes = [\"default\"]", "proof_modes = []"),
            "engines[0].proof_modes",
        ),
        (
            VALID_MANIFEST.replace(
                "proof_modes = [\"default\"]",
                "proof_modes = [\"default\", \"default\"]",
            ),
            "engines[0].proof_modes[1]",
        ),
        (
            VALID_MANIFEST.replace(
                "commit_output = \"bytes\"",
                "commit_output = \"bytes\"\nsurprise = true",
            ),
            "workloads[0].inputs[0].surprise",
        ),
        (
            VALID_MANIFEST.replace("fixture = \"input.bin\"\n", ""),
            "workloads[0].inputs[0].fixture",
        ),
    ];

    for (source, expected_path) in cases {
        let temporary = TemporaryManifest::new(&source);
        let error = BenchmarkManifest::load(temporary.path())
            .expect_err("mutated manifest should be rejected");
        assert_eq!(
            error.field_path(),
            expected_path,
            "unexpected diagnostic for {expected_path}: {error}"
        );
    }
}

#[test]
fn duplicate_input_and_engine_ids_are_rejected() {
    let duplicate_input = VALID_MANIFEST.replace(
        "[[engines]]",
        r#"[[workloads.inputs]]
id = "small"
fixture = "input.bin"
expected_output = "output.bin"
visibility = "private"
preprocessing = "none"
commit_input = "digest"
commit_output = "bytes"

[[engines]]"#,
    );
    let temporary = TemporaryManifest::new(&duplicate_input);
    let error =
        BenchmarkManifest::load(temporary.path()).expect_err("duplicate input should be rejected");
    assert_eq!(error.field_path(), "workloads[0].inputs[1].id");

    let duplicate_engine = format!(
        r#"{VALID_MANIFEST}

[[engines]]
id = "mock"
adapter = "mock.zkperf-adapter.json"
proof_modes = ["default"]
"#
    );
    let temporary = TemporaryManifest::new(&duplicate_engine);
    let error =
        BenchmarkManifest::load(temporary.path()).expect_err("duplicate engine should be rejected");
    assert_eq!(error.field_path(), "engines[1].id");
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("manifest")
}

struct TemporaryManifest {
    root: PathBuf,
}

impl TemporaryManifest {
    fn new(source: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let root = std::env::temp_dir().join(format!(
            "zkperf-manifest-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("temporary manifest directory should be unique");
        for file in SUPPORT_FILES {
            fs::copy(fixture_root().join(file), root.join(file))
                .expect("support fixture should copy");
        }
        fs::write(root.join("zkperf.toml"), source).expect("temporary manifest should write");
        Self { root }
    }

    fn path(&self) -> PathBuf {
        self.root.join("zkperf.toml")
    }
}

impl Drop for TemporaryManifest {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("temporary manifest directory should clean up");
    }
}
