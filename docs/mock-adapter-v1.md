# Deterministic mock adapter v1

Issue #12 exercises the existing subprocess protocol and CLI without a zkVM SDK.
The adapter is a standalone Python 3.10+ program using only the standard library.
It runs once per request, implements all six operations, and advertises separate
build, setup, execution, proving, compression, and verification boundaries.

The fixture hashes the exact input bytes with SHA-256. Its canonical output is
the lowercase hexadecimal digest followed by one LF byte. Execution computes
this output and checks it against the benchmark's expected output digest.
Requested input/output commitments are hashes of the actual bytes. Preparation
artifacts bind their stage and benchmark. Execution traces retain the input and
benchmark; initial proofs seal that statement and witness with a domain-separated
hash. The transformation compresses the initial proof with zlib. Verification
checks the seal, recomputes the workload, and compares the benchmark, output,
commitments, and supplied application statement. These are test artifacts with
no cryptographic proof security, regardless of the benchmark security target.

Artifacts contain no request IDs, host paths, clocks, or process IDs. For the
same benchmark, input, and configuration, artifacts and result fields are byte reproducible on
the same Python/zlib versions across fresh workspaces. Timing, resource usage,
run identities, and evidence paths are intentionally not reproducible.

Fault controls live in `engines.configuration.mock`; command-line controls can
also target bootstrap capabilities, whose request has no engine configuration.
A target is `all`, an operation, or `operation.stage` (for example
`prove.initial`). Delay and touched resident memory apply only to that target.
Timeouts wait for host cancellation; forced timeouts ignore cancellation.
Normal delays poll the correlated control file and return a structured cancelled
error. Memory stays allocated until the operation ends. Faults must not change
normal artifact content or silently leak into other stages.

## Running the fixture

From the repository root, create a benchmark in a new or empty directory:

```console
python3 tools/create_mock_fixture.py ../zkperf-mock-benchmark
cargo run -p zkperf-cli -- run --manifest ../zkperf-mock-benchmark/zkperf.toml
```

Use `python` on Windows. The generator copies the checked-in fixture bytes and
writes `mock.zkperf-adapter.json` with the current Python executable and adapter
script as absolute paths. It refuses to overwrite existing files. The adapter
command uses `-B` to avoid bytecode-cache writes outside the operation workspace.
Regenerate into an empty directory after moving the checkout or interpreter.
No package installation, SDK, network access, or inherited environment is needed
to run the adapter. The CLI retains operation evidence under the generated
benchmark's `results/` directory; it does not yet produce aggregate reports.

Change the generated manifest's existing mock table to inject a fault:

```toml
[engines.configuration.mock]
target = "prove.initial"
fault = "error"
delay_ms = 100
memory_bytes = 1048576
```

| Control | Behavior |
| --- | --- |
| `target` | `all`, one of the six operations, or `prepare.environment`, `prepare.build`, `prepare.setup`, `prove.initial`, `prove.transform`. Default: `all`. |
| `delay_ms` | Sleep before the operation, polling cancellation. Integer 0–3,600,000; default 0. |
| `memory_bytes` | Allocate, touch, and retain this many bytes during delay and execution. Integer 0–268,435,456; default 0. This excludes interpreter and workload overhead. |
| `fault` | One of the faults below; default `none`. |

| Fault | Observable behavior |
| --- | --- |
| `error`, `unsupported` | Correlated structured response with the operation's failure phase. |
| `exit`, `signal` | Exit 7 or terminate with SIGTERM (signal identity is platform dependent). |
| `malformed`, `invalid-utf8`, `trailing`, `empty` | Invalid framing or missing stdout. |
| `mismatch`, `schema` | Wrong request ID or missing required response field. |
| `stdout-overflow` | Emit two MiB, exceeding the one MiB protocol limit. |
| `stderr-flood` | Emit two MiB of logs and complete successfully; the host truncates logs. |
| `missing-artifact`, `bad-digest`, `bad-length`, `artifact-escape` | Remove an output, corrupt its bytes, append a byte, or return a traversal path. These require an artifact-producing operation. No escaped file is created. |
| `timeout` | Wait until a correlated host cancellation notice, then acknowledge it. The host retains `timed_out` after its deadline. |
| `ignore-cancel` | Wait indefinitely, requiring the host to terminate the process after grace. |
| `blocked-input` | Command-line only: never read stdin, exercising transport timeout. It applies before operation/target decoding. |

The adapter also accepts `--fault`, `--target`, `--delay-ms`, and `--memory-bytes`
as literal command arguments. Explicit command-line values override the matching
manifest settings. For example, append `"--fault", "malformed", "--target",
"capabilities"` to the adapter manifest's command to fail bootstrap negotiation.
Unknown targets, options, and out-of-range resource settings are errors.

The mock's application statement is an object containing `workload` (the case
ID), `input_commitment`, and `output_commitment` (digest hex strings, or JSON null
when that commitment is disabled). Verification requires an exact match.
Protocol inputs are bounded to one MiB of JSON; each input/output artifact is
bounded to 16 MiB, and the advertised response limits are 32 artifacts and 32 MiB
total. Proofs retain the input as hex, so usable workload inputs are smaller
than the artifact limit. Malformed envelopes or unsupported protocol versions
exit 64 with stderr and no stdout; rejected operations return structured errors.

## Verification

The focused tests cover every operation and preparation/proving stage, schema
and semantic conformance, byte reproducibility across changed request IDs and
paths, input/proof/statement tampering, commitment policies, and retained CLI
outcomes for structured errors, unsupported operations, process failures,
malformed protocol, invalid artifacts, timeout, and cancellation. Existing
runner-only fixtures continue covering platform-specific descendant handling
and harness failures such as missing executables and evidence-storage errors.

```console
python3 -m pip install -r requirements-dev.txt
python3 -m unittest discover -s tests -p 'test_mock*.py' -v
cargo test -p zkperf-cli --test mock_adapter --locked
```

Conformance uses the repository's checked-in schema and semantic validator.
After installing the development dependencies, all these tests run offline.
CI discovers the Python tests and Rust integration target through the existing
quality gate; the adapter adds no Rust dependencies.
