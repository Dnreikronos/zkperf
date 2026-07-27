use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use zkperf_core::{
    BenchmarkManifest, ConcurrencyMode, ExecutionMode, ManifestVersion, OutputFormat,
    PercentileMethod, StandardPhase,
};

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
    assert_eq!(first.manifest_version(), ManifestVersion::V1_0_0);
    assert_eq!(first.run().warmups(), 1);
    assert_eq!(first.run().runs().get(), 3);
    assert_eq!(
        first.run().policy().ordering_algorithm().as_str(),
        "seeded_round_robin_v1"
    );
    assert_eq!(first.run().policy().seed(), 42);
    assert_eq!(
        first.run().policy().concurrency().mode(),
        ConcurrencyMode::Isolated
    );
    assert_eq!(
        first.run().policy().concurrency().parallel_attempts().get(),
        1
    );
    assert_eq!(
        first.run().policy().percentile_method(),
        PercentileMethod::Linear
    );
    assert_eq!(first.run().resources().worker_count().get(), 1);
    assert_eq!(
        first.run().resources().execution_mode(),
        ExecutionMode::Local
    );
    assert_eq!(
        first.outputs().formats(),
        &[OutputFormat::Json, OutputFormat::Terminal]
    );
    assert_eq!(
        first.workloads()[0].phases(),
        &[
            StandardPhase::Execution,
            StandardPhase::Proving,
            StandardPhase::Verification,
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
    let debug = manifest
        .normalized_debug()
        .expect("validated manifest should serialize");
    let mut normalized: Value =
        serde_json::from_str(&debug).expect("normalized debug output should be valid JSON");
    assert_eq!(
        normalized,
        serde_json::to_value(&manifest).expect("validated manifest should serialize")
    );
    normalize_snapshot_paths(&mut normalized, &root.to_string_lossy());
    let snapshot: Value = serde_json::from_str(include_str!("snapshots/manifest_normalized.json"))
        .expect("manifest snapshot should be valid JSON");

    assert_eq!(normalized, snapshot);
}

#[test]
fn snapshot_normalization_only_rewrites_rooted_paths() {
    let root = r"\\?\D:\a\zkperf";
    let encoded = serde_json::to_string_pretty(&serde_json::json!({
        "manifest_path": r"\\?\D:\a\zkperf\zkperf.toml",
        "configuration": {
            "pattern": r"\d+",
        },
    }))
    .expect("synthetic snapshot should serialize")
    .replace('\n', "\r\n");
    let mut normalized: Value =
        serde_json::from_str(&encoded).expect("CRLF JSON should deserialize");
    normalize_snapshot_paths(&mut normalized, root);

    assert_eq!(
        normalized,
        serde_json::json!({
            "manifest_path": "$FIXTURE_ROOT/zkperf.toml",
            "configuration": {
                "pattern": r"\d+",
            },
        })
    );
}

#[test]
fn manifest_version_errors_use_manifest_domain_language() {
    let source = VALID_MANIFEST.replace(
        "manifest_version = \"1.0.0\"",
        "manifest_version = \"2.0.0\"",
    );
    let temporary = TemporaryManifest::new(&source);
    let error = BenchmarkManifest::load(temporary.path())
        .expect_err("unsupported manifest version should be rejected");

    assert_eq!(error.field_path(), "manifest_version");
    assert!(
        error
            .to_string()
            .contains("unsupported benchmark manifest version 2.0.0")
    );
    assert!(!error.to_string().contains("BenchmarkReport"));
}

#[test]
fn validation_errors_identify_exact_field_paths() {
    let cases = [
        (VALID_MANIFEST.replace("runs = 3", "runs = 0"), "run.runs"),
        (
            VALID_MANIFEST.replace(
                "ordering_algorithm = \"seeded_round_robin_v1\"",
                "ordering_algorithm = \"\"",
            ),
            "run.policy.ordering_algorithm",
        ),
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
            VALID_MANIFEST.replacen("fixture = \"input.bin\"", "", 1),
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
fn run_policy_and_resources_reject_nonconforming_values_precisely() {
    let cases = [
        (
            VALID_MANIFEST.replace(
                "retry_policy = \"no retries except replacements for invalid attempts\"",
                "retry_policy = \"retry engine errors\"",
            ),
            "run.policy.retry_policy",
        ),
        (
            VALID_MANIFEST.replace(
                "invalidation_policy = \"replace only verified harness or external interference\"",
                "invalidation_policy = \"drop slow attempts\"",
            ),
            "run.policy.invalidation_policy",
        ),
        (
            VALID_MANIFEST.replace("outlier_rule = \"none\"", "outlier_rule = \"trim_top_1\""),
            "run.policy.outlier_rule",
        ),
        (
            VALID_MANIFEST.replace("parallel_attempts = 1", "parallel_attempts = 2"),
            "run.policy.concurrency.parallel_attempts",
        ),
        (
            VALID_MANIFEST.replace("cpu_affinity = [0]", "cpu_affinity = [0, 0]"),
            "run.resources.cpu_affinity[1]",
        ),
        (
            VALID_MANIFEST.replace(
                "environment_variables = {}",
                "environment_variables = { API_TOKEN = \"secret\" }",
            ),
            "run.resources.environment_variables.API_TOKEN",
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

#[test]
fn remote_resource_profiles_are_loaded_and_exposed() {
    let source =
        VALID_MANIFEST.replace("execution_mode = \"local\"", "execution_mode = \"remote\"");
    let source = source.replace(
        "network_access = false",
        r#"network_access = true

[run.resources.remote]
endpoint = "https://prover.example/v1"
client_region = "us-east-1"
endpoint_region = "us-west-2"
transport = "https"
connection_type = "public-internet"
round_trip_latency_ns = 12_000_000
jitter_ns = 400_000
packet_loss_ratio = 0.01
available_bandwidth_bytes_per_second = 125_000_000
measurement_method = "iperf3"
measured_at = "2026-07-26T00:00:00Z""#,
    );
    let temporary = TemporaryManifest::new(&source);
    let manifest = BenchmarkManifest::load(temporary.path()).expect("remote manifest should load");
    let remote = manifest
        .run()
        .resources()
        .remote()
        .expect("remote execution must expose its profile");

    assert_eq!(
        manifest.run().resources().execution_mode(),
        ExecutionMode::Remote
    );
    assert_eq!(remote.endpoint().as_str(), "https://prover.example/v1");
    assert_eq!(remote.client_region().as_str(), "us-east-1");
    assert_eq!(remote.endpoint_region().as_str(), "us-west-2");
    assert_eq!(remote.round_trip_latency_ns().get(), 12_000_000);
    assert_eq!(
        remote.available_bandwidth_bytes_per_second().get(),
        125_000_000
    );
    assert_eq!(remote.packet_loss_ratio().as_number().to_string(), "0.01");
}

#[test]
fn normalized_debug_redacts_environment_values() {
    let source = VALID_MANIFEST.replace(
        "environment_variables = {}",
        "environment_variables = { RUST_LOG = \"trace\" }",
    );
    let temporary = TemporaryManifest::new(&source);
    let manifest =
        BenchmarkManifest::load(temporary.path()).expect("manifest with env should load");
    let debug = manifest
        .normalized_debug()
        .expect("validated manifest should serialize");
    let normalized: Value =
        serde_json::from_str(&debug).expect("normalized debug output should be valid JSON");

    assert_eq!(
        normalized["run"]["resources"]["environment_variables"]["RUST_LOG"],
        "[redacted]"
    );
    assert!(!debug.contains("trace"));
}

#[test]
fn secret_like_configuration_keys_are_rejected() {
    let cases = [
        (
            VALID_MANIFEST.replace("backend = \"cpu\"", "api_key = \"secret\""),
            "engines[0].configuration.api_key",
        ),
        (
            VALID_MANIFEST.replace("backend = \"cpu\"", "settings = { apiKey = \"secret\" }"),
            "engines[0].configuration.settings.apiKey",
        ),
        (
            VALID_MANIFEST.replace(
                "backend = \"cpu\"",
                "settings = [{ privateKey = \"secret\" }]",
            ),
            "engines[0].configuration.settings[0].privateKey",
        ),
        (
            VALID_MANIFEST.replace("backend = \"cpu\"", "settings = { accesskey = \"secret\" }"),
            "engines[0].configuration.settings.accesskey",
        ),
    ];

    for (source, expected_path) in cases {
        let temporary = TemporaryManifest::new(&source);
        let error = BenchmarkManifest::load(temporary.path())
            .expect_err("secret-like configuration keys should be rejected");

        assert_eq!(error.field_path(), expected_path);
    }
}

#[cfg(windows)]
#[test]
fn windows_prefixed_paths_cannot_escape_the_manifest_directory() {
    for fixture in [r#"fixture = "C:input.bin""#, r#"fixture = '\input.bin'"#] {
        let source = VALID_MANIFEST.replacen(r#"fixture = "input.bin""#, fixture, 1);
        let temporary = TemporaryManifest::new(&source);
        let error = BenchmarkManifest::load(temporary.path())
            .expect_err("Windows-prefixed fixture should be rejected");
        assert_eq!(error.field_path(), "workloads[0].inputs[0].fixture");
    }
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("manifest")
}

fn normalize_snapshot_paths(value: &mut Value, root: &str) {
    normalize_snapshot_paths_at(value, root, &[]);
}

fn normalize_snapshot_paths_at(value: &mut Value, root: &str, path: &[&str]) {
    match value {
        Value::String(text) => {
            if is_snapshot_path_field(path) {
                if let Some(relative) = text.strip_prefix(root) {
                    *text = format!("$FIXTURE_ROOT{}", relative.replace('\\', "/"));
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_snapshot_paths_at(value, root, path);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let mut field_path = path.to_vec();
                field_path.push(key);
                normalize_snapshot_paths_at(value, root, &field_path);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_snapshot_path_field(path: &[&str]) -> bool {
    matches!(
        path,
        ["manifest_path"]
            | ["outputs", "directory"]
            | ["workloads", "specification"]
            | ["workloads", "inputs", "fixture" | "expected_output"]
            | ["engines", "adapter"]
    )
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
