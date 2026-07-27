# Benchmark manifest v1

## 1. Purpose

`zkperf.toml` is the versioned, user-authored source for benchmark cases. The
loader turns it into a validated, engine-independent definition before planning
or adapter discovery begins.

Manifest version `1.0.0` has no implicit policy defaults. Counts, timeouts,
inputs, engines, proof-mode selections, and outputs are explicit so that the
same file produces the same benchmark definition on every supported host.

## 2. Example

```toml
manifest_version = "1.0.0"

[run]
warmups = 1
runs = 10

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
revision = "v1"
specification = "workloads/sha256.md"
implementation_lane = "portable"
security_target_bits = 128
phases = ["execution", "proving", "verification"]

[[workloads.inputs]]
id = "small"
fixture = "fixtures/input.bin"
expected_output = "fixtures/output.bin"
visibility = "private"
preprocessing = "none"
commit_input = "digest"
commit_output = "bytes"

[[engines]]
id = "mock"
adapter = "adapters/mock.zkperf-adapter.json"
proof_modes = ["default"]

[engines.configuration]
workers = 1
```

## 3. Fields

All tables reject unknown fields.

- `manifest_version` is required and must be `1.0.0`.
- `run.warmups` is a non-negative integer. `run.runs` is a positive integer.
- Every `run.timeouts` entry selects one phase and supplies a positive
  `limit_ms`. `termination_grace_ms` may be zero but cannot exceed the limit.
- `outputs.directory` is a manifest-relative directory. `outputs.formats`
  contains one or more of `terminal`, `json`, `html`, or `csv`.
- Every workload has a unique slug `id`, a non-empty revision, a regular-file
  specification, an implementation lane, a positive security target, one or
  more phases, and one or more inputs.
- Every input has an ID unique within its workload, canonical input and expected
  output fixtures, visibility, a non-empty preprocessing policy, and explicit
  input/output commitment policies.
- Every engine has a unique slug `id`, an adapter manifest file, a unique list
  of adapter proof-mode IDs, and an optional free-form configuration table.

Supported phases are `build`, `setup`, `execution`, `proving`, `compression`,
`verification`, and `end_to_end`. A timeout must exist for every phase selected
by any workload. Proof-related phases require at least one proof mode on every
engine.

## 4. Path and fixture rules

Paths in the manifest must be relative. The loader resolves them against the
directory containing `zkperf.toml`, not the process working directory.

The manifest itself and every workload specification, input fixture, expected
output fixture, and adapter manifest must resolve to an existing regular file.
The output directory may be absent, but it must not resolve to an existing
non-directory. Resolved fixture files expose a SHA-256 content digest; paths
alone are never used as content identity.

## 5. Validation and diagnostics

Deserialization, path resolution, and cross-field validation are one operation.
Failures carry the most specific available field path, using zero-based array
indices such as `workloads[1].inputs[0].fixture`.

Validation rejects:

- missing or unknown fields and unsupported manifest versions;
- empty collections where the schema requires a selection;
- duplicate workload, input, engine, proof-mode, phase, timeout, or output
  format values;
- invalid slugs, empty text, zero measured-run counts, zero timeout limits, and
  termination grace periods larger than their limits;
- absolute paths, missing or non-file fixtures, and output paths that are
  existing non-directories;
- missing timeout policies for selected phases; and
- proof-related workloads paired with an engine that selects no proof mode.

## 6. Normalized debug representation

The validated definition has a deterministic, pretty-printed JSON debug
representation. It uses canonical manifest and fixture paths, stable object-key
ordering, explicit values, and the original order of user-selected arrays. It
contains no process-specific debug formatting and is suitable for snapshots and
`zkperf validate` diagnostics.
