// Keep the manifest last so fixtures exist before it is published.
pub(super) const FILES: &[(&str, &[u8])] = &[
    ("benchmarks/sha256.md", WORKLOAD.as_bytes()),
    ("benchmarks/input.bin", b"abc"),
    (
        "benchmarks/output.bin",
        &[
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ],
    ),
    ("benchmarks/mock.zkperf-adapter.json", ADAPTER.as_bytes()),
    ("zkperf.toml", MANIFEST.as_bytes()),
];

const WORKLOAD: &str = "# SHA-256 starter workload\n\n\
Hash the exact input bytes with SHA-256, without preprocessing.\n\
The input is the three ASCII bytes `abc` (no newline); the expected output\n\
is the raw 32-byte digest, not its hexadecimal encoding.\n\n\
This is a configuration starter. Benchmark execution and the mock adapter\n\
executable are pending issues #9–15. The descriptor expects\n\
`zkperf-adapter-mock` on PATH when execution is available.\n";

const ADAPTER: &str = r#"{
  "kind": "zkperf-adapter-manifest",
  "manifest_version": "1.0.0",
  "adapter_id": "mock",
  "display_name": "Mock adapter",
  "command": ["zkperf-adapter-mock"],
  "protocol_versions": ["1.0.0"]
}
"#;

const MANIFEST: &str = r#"manifest_version = "1.0.0"

[run]
warmups = 1
runs = 3

[run.policy]
ordering_algorithm = "seeded_round_robin_v1"
seed = 42
retry_policy = "no retries except replacements for invalid attempts"
invalidation_policy = "replace only verified harness or external interference"
outlier_rule = "none"
percentile_method = "linear"

[run.policy.concurrency]
mode = "isolated"
parallel_attempts = 1

[run.resources]
power_profile = "performance"
cpu_affinity = [0]
cpu_limit = 1
memory_limit_bytes = 1_073_741_824
worker_count = 1
accelerator_allocation = []
environment_variables = {}
execution_mode = "local"
network_access = false

[[run.timeouts]]
phase = "execution"
limit_ms = 30_000
termination_grace_ms = 1_000

[[run.timeouts]]
phase = "proving"
limit_ms = 300_000
termination_grace_ms = 5_000

[[run.timeouts]]
phase = "verification"
limit_ms = 30_000
termination_grace_ms = 1_000

[outputs]
directory = "runs"
formats = ["terminal", "json"]

[[workloads]]
id = "sha256"
revision = "starter-v1"
specification = "benchmarks/sha256.md"
implementation_lane = "portable"
security_target_bits = 128
phases = ["execution", "proving", "verification"]

[[workloads.inputs]]
id = "small"
fixture = "benchmarks/input.bin"
expected_output = "benchmarks/output.bin"
visibility = "private"
preprocessing = "none"
commit_input = "digest"
commit_output = "bytes"

[[engines]]
id = "mock"
adapter = "benchmarks/mock.zkperf-adapter.json"
proof_modes = ["default"]

[engines.configuration]
workers = 1
"#;
