# zkperf Adapter Subprocess Protocol v1

| Field | Value |
| --- | --- |
| Protocol ID | `zkperf-adapter` |
| Protocol version | `1.0.0` |
| Manifest version | `1.0.0` |
| Status | Draft |
| JSON Schema | [`../schemas/adapter-protocol-v1.schema.json`](../schemas/adapter-protocol-v1.schema.json) |
| Examples | [`../examples/protocol-v1.json`](../examples/protocol-v1.json) |
| Fairness contract | [`benchmark-fairness-contract-v1.md`](benchmark-fairness-contract-v1.md) |
| Source issue | `Dnreikronos/zkperf#3` |

This document defines the process boundary between zkperf and an engine
adapter. An adapter isolates an engine SDK, toolchain, and engine-specific
dependencies from the zkperf core. The protocol is deliberately
request/response and process-per-operation: zkperf starts a fresh adapter
process for one operation, writes one request, reads one response, waits for
the process to exit, validates the response and its artifacts, and only then
continues.

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT,
RECOMMENDED, MAY, and OPTIONAL are to be interpreted as described in RFC 2119
and RFC 8174 when they appear in uppercase.

## 1. Design constraints

The protocol has six operations:

| Operation | Purpose |
| --- | --- |
| `capabilities` | Confirm protocol compatibility and describe supported operations, boundaries, proof modes, cancellation, and limits. |
| `metadata` | Report adapter, engine, SDK, toolchain, proof-system, verifier, and backend identity. |
| `prepare` | Perform untimed environment preparation or the separately measurable `build` and `setup` stages. |
| `execute` | Execute the canonical workload and materialize its canonical output and execution result. |
| `prove` | Produce either the first independently verifiable proof or an optional proof transformation. |
| `verify` | Check the complete application statement for a selected proof. |

One invocation performs exactly one operation. A request MUST NOT ask an
adapter to execute an implicit multi-operation workflow. zkperf may measure an
`end_to_end` phase by supervising the required operation sequence as one
aggregate, but the component requests and artifacts remain explicit.

An adapter is the only component that links to or invokes an engine SDK.
Protocol messages MUST contain JSON-compatible data and artifact references;
they MUST NOT contain engine objects, handles, pointers, file descriptors, or
embedded binary artifacts.

## 2. Discovery

### 2.1 Manifest

An adapter is advertised by a UTF-8 JSON manifest whose file name ends in
`.zkperf-adapter.json`. The manifest is validated against
`adapterManifest` in the protocol schema before its command is considered.

```json
{
  "kind": "zkperf-adapter-manifest",
  "manifest_version": "1.0.0",
  "adapter_id": "mock",
  "display_name": "zkperf deterministic mock adapter",
  "command": ["./bin/zkperf-adapter-mock"],
  "protocol_versions": ["1.0.0"]
}
```

zkperf discovers manifests only from:

1. adapter manifests named explicitly by the benchmark manifest or CLI;
2. directories named explicitly by the benchmark manifest or CLI; and
3. implementation-defined user and system adapter directories documented by
   zkperf.

zkperf MUST NOT recursively scan the current directory or execute arbitrary
commands found on `PATH` as discovery. Duplicate `adapter_id` values are an
error unless the user explicitly selects one manifest.

The first command element may be absolute or relative. A relative executable
is resolved against the directory containing the adapter manifest. Remaining
elements are literal arguments. Environment-variable expansion, shell
evaluation, globbing, and command substitution are forbidden. zkperf MUST
invoke the command directly without a shell.

Discovery does not establish trust. Selecting an adapter authorizes execution
of that adapter with the permissions of the zkperf process. zkperf SHOULD show
the resolved manifest and executable when validating configuration and SHOULD
warn when either is writable by an unexpected principal.

### 2.2 Runtime confirmation

The manifest is a static discovery hint, not an authoritative capability
record. Before any other operation, zkperf MUST invoke `capabilities` using the
selected protocol version. The response MUST:

- identify the same `adapter_id`;
- include the selected version in `supported_protocol_versions`;
- describe all six protocol operations;
- enumerate every supported proof mode; and
- describe each observable measurement boundary.

A disagreement between the manifest and the response is a
`capability_mismatch` protocol failure. zkperf MUST NOT silently use the
manifest value in place of the runtime value.

## 3. Version negotiation

Protocol and manifest versions use Semantic Versioning.

1. zkperf validates `manifest_version` independently from
   `protocol_versions`.
2. zkperf constructs the exact intersection of the protocol versions it
   implements and the versions listed by the manifest.
3. zkperf selects the highest exact version in that intersection.
4. zkperf sends that exact value in `protocol_version` on every request.
5. The adapter repeats the exact value on every response.

The runtime capability list confirms that selection; it does not participate
in choosing a lower version. If the runtime response omits the selected
highest exact version, manifest/runtime capability agreement fails.

A peer supports only versions it explicitly lists. Support for `1.1.0` does
not imply support for `1.0.0`, even when the versions are wire-compatible.
Peers SHOULD list every compatible version they implement. This exact
selection rule avoids guessing about features or unknown enum values.

Version changes follow these rules:

- A MAJOR version may remove fields, alter operation semantics, or change
  framing. Different MAJOR versions are incompatible.
- A MINOR version may add optional fields, optional operations within an
  existing operation, proof features, or enum values without changing
  existing meaning.
- A PATCH version may clarify constraints or fix a schema defect without
  changing valid message meaning.

Every published schema version is immutable. Version selection happens before
runtime work. If manifest drift or a host defect nevertheless sends an
unsupported version, the adapter exits `64`, writes a diagnostic to stderr,
and emits no versioned response because there is no negotiated schema under
which that response could be valid. zkperf MUST NOT retry the same request
under another version after benchmark work has begun.

## 4. Invocation and framing

### 4.1 Process contract

For each operation, zkperf:

1. creates an isolated operation workspace;
2. sets that workspace as the process working directory;
3. starts the adapter command directly;
4. writes exactly one request to standard input and closes standard input;
5. reads standard output and bounded standard error concurrently;
6. waits for the adapter and its inherited output handles to close;
7. validates the exit status, response, and referenced output artifacts; and
8. retains the request, response bytes, logs, process outcome, and artifacts
   as run evidence.

Adapters MUST NOT daemonize, detach descendants, or leave child processes
running after the response process exits.

The environment is allowlist-based. zkperf MUST provide only the variables
needed for the selected engine, platform operation, and deterministic
execution. Secrets MUST NOT be placed in requests, responses, manifests, or
captured logs.

### 4.2 Standard input and output

The request and response are each one UTF-8 encoded JSON object. Leading and
trailing JSON whitespace is allowed. A trailing newline is RECOMMENDED. A
byte-order mark, multiple JSON values, non-whitespace before or after the JSON
object, invalid UTF-8, or output exceeding the negotiated limit makes the
response malformed.

Standard output is reserved exclusively for the response. Adapter status
messages, SDK output, compiler output, progress bars, and diagnostics MUST be
written to standard error or an artifact. An adapter MUST redirect the stdout
of any child that cannot honor this rule.

The response is not a streaming protocol. An adapter MUST completely
materialize and close every referenced artifact before writing a success
response.

The initial `capabilities` invocation is bounded by zkperf's documented
bootstrap stdout limit because no adapter limit has been read yet. After a
successful capability exchange, the effective stdout and artifact limits are
the stricter of zkperf policy and the adapter's declared limits.

### 4.3 Standard error

Standard error is the log channel. It SHOULD be UTF-8 text with one logical
record per line, but it is not parsed as protocol data. zkperf captures stderr
as bounded bytes, records whether it was truncated, and may persist the full
log when policy permits. Truncating logs MUST NOT block a child process.

Logs MUST NOT be copied into stdout. A valid response remains valid when
stderr is non-empty.

### 4.4 Exit status

Exit code `0` means that the adapter completed the protocol transaction and
emitted one schema-valid response. This includes `success`, `unsupported`, and
structured `error` responses.

Exit code `64` means the request could not be decoded sufficiently to emit a
correlated response. The adapter writes diagnostics to stderr and MUST NOT
write a partial response to stdout.

Any other non-zero exit, signal termination, missing exit status, or process
that outlives supervision is an adapter process failure. If stdout also
contains a response, zkperf retains it as diagnostic evidence but MUST NOT
treat it as a successful transaction.

## 5. Message envelopes

### 5.1 Request

Every request contains:

```json
{
  "protocol": "zkperf-adapter",
  "protocol_version": "1.0.0",
  "request_id": "10000000-0000-4000-8000-000000000001",
  "operation": "capabilities",
  "workspace": {
    "inputs_dir": "inputs",
    "outputs_dir": "outputs",
    "control_dir": "control"
  },
  "timeout": {
    "limit_ns": 30000000000,
    "termination_grace_ns": 1000000000
  },
  "params": {
    "host": {
      "name": "zkperf",
      "version": "0.1.0",
      "supported_protocol_versions": ["1.0.0"]
    }
  }
}
```

`request_id` is unique within a run. `workspace` contains paths relative to
the process working directory. `timeout.limit_ns` is the operation limit and
`termination_grace_ns` is cleanup time after timeout or cancellation; grace
time is never benchmark duration.

### 5.2 Response

Every response repeats the correlation fields. This complete structured-error
example shows the common envelope; successful operation-specific results are
in the checked-in examples:

```json
{
  "protocol": "zkperf-adapter",
  "protocol_version": "1.0.0",
  "request_id": "10000000-0000-4000-8000-000000000009",
  "operation": "prove",
  "status": "error",
  "error": {
    "phase": "proving",
    "code": "engine_prover_failed",
    "message": "The engine prover returned status 2.",
    "retryable": false
  },
  "artifacts": []
}
```

The response `request_id`, `operation`, and `protocol_version` MUST exactly
match the request. `artifacts` is always present, including when empty.
Artifact identifiers are unique within a response.

`status` has three values:

| Status | Meaning |
| --- | --- |
| `success` | The operation completed and its operation-specific invariants hold. |
| `unsupported` | The request is valid, but the negotiated adapter does not support the requested optional operation, stage, proof mode, or feature. |
| `error` | The adapter attempted or validated the operation and it failed. |

`success` requires `result` and forbids `error`. `unsupported` and `error`
require `error` and forbid `result`. Unsupported behavior is capability
information, not an engine failure, and MUST be preserved as such when mapped
into `BenchmarkReport`.

### 5.3 Errors

An error contains a stable code, a human-readable message, a failure phase,
and a retryability declaration:

```json
{
  "phase": "proving",
  "code": "engine_prover_failed",
  "message": "The engine prover returned status 2.",
  "retryable": false,
  "details": {
    "engine_status": 2
  }
}
```

`details` is optional, non-secret diagnostic data. Consumers MUST use `code`
and `phase` for decisions and treat `message` as presentation text.
`retryable: true` is advisory only; zkperf's predeclared retry policy remains
authoritative.

## 6. Workspaces and artifacts

### 6.1 Directory ownership

The working directory contains three host-created roots:

| Root | Adapter access |
| --- | --- |
| `inputs_dir` | Read-only protocol inputs. |
| `outputs_dir` | Adapter output. The adapter MUST create output artifacts only here. |
| `control_dir` | Host-owned cancellation and control files. The adapter MUST NOT modify it. |

All paths in messages use `/` separators, are relative, and MUST NOT contain an
empty segment, `.` segment, `..` segment, backslash, or NUL. A trailing `/`
creates an empty final segment and is invalid. Before opening a path, both
peers MUST resolve it against the applicable root and reject any escape,
including an escape through a symbolic link, junction, mount, or
platform-specific alias.

The three workspace roots MUST be pairwise disjoint. Two roots are not
disjoint when they are equal or when either is a path ancestor of the other.
This keeps read-only inputs, adapter-owned outputs, and host-owned control
files under separate ownership.

Input artifacts are immutable for one invocation. An adapter MUST NOT update
them in place. Output artifacts SHOULD be written to a temporary name inside
`outputs_dir`, flushed as required by the platform, and atomically renamed to
their response path.

### 6.2 Artifact reference

Binary and potentially large values cross the boundary by reference:

```json
{
  "id": "canonical-output",
  "kind": "canonical_output",
  "path": "outputs/canonical-output.bin",
  "media_type": "application/octet-stream",
  "byte_length": 32,
  "digest": {
    "algorithm": "sha256",
    "value": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  }
}
```

The host validates the path, file type, byte length, and digest before
accepting the response. Referenced files MUST be regular files. A missing,
unreadable, mutable, oversized, wrong-length, or wrong-digest artifact changes
the operation outcome to an artifact validation failure even if the response
said `success`.

Operation results refer to response artifacts by `id`; they do not duplicate
artifact metadata. This is the single source of truth for output paths,
lengths, media types, and digests.

Artifacts in a request use the same shape. Their paths MUST be under
`inputs_dir`. Artifacts in a response MUST be under `outputs_dir`.

Artifact references also have operation-specific kinds:

| Field | Required referenced kind |
| --- | --- |
| `execute.params.canonical_input` | `canonical_input` |
| `execute.result.canonical_output_artifact_id` | `canonical_output` |
| `execute.result.execution_artifact_id` | `execution_trace` |
| `prove.params.input_artifacts` for `initial` | At least one `execution_trace` |
| `prove.params.input_artifacts` for `transform` | At least one `proof` |
| `prove.result.proof_artifact_id` | `proof` |
| `prove.result.public_values_artifact_id` | `public_values` |
| `verify.params.proof` | `proof` |

An identifier that resolves to an artifact of the wrong kind is invalid. Mere
identifier existence is not sufficient.

Only fields defined by an operation as artifact references participate in
artifact ownership and validation. Artifact-shaped objects inside
`configuration`, `statement`, `extensions`, or other opaque JSON remain
namespaced data and MUST NOT be interpreted as protocol artifacts.

## 7. Operations

The complete, schema-validated exchanges are in
[`examples/protocol-v1.json`](../examples/protocol-v1.json).

### 7.1 `capabilities`

`capabilities` is the first runtime operation. The request identifies zkperf
and all protocol versions the host implements. The response reports:

- adapter identity and implemented versions;
- support and optional stages for every operation;
- observable operation-to-measurement boundaries;
- proof modes and optional transformations;
- graceful cancellation support; and
- protocol stdout and artifact size limits.

Capabilities describe what an adapter implementation can do, not whether a
particular engine configuration is currently installed. Missing dependencies
belong in `metadata` or the affected operation as a structured error.

After capability negotiation, every response MUST respect the advertised
artifact limits. The number of response artifacts MUST NOT exceed
`max_artifact_count`; each declared `byte_length` MUST NOT exceed
`max_artifact_bytes`; and their sum MUST NOT exceed
`max_total_artifact_bytes`. Filesystem validation later confirms that the
declared lengths match the materialized files.

Each boundary maps one adapter invocation to either one standard fairness
phase or the smallest observable ordered combination. For example, an engine
that cannot separate execution and proving advertises:

```json
{
  "operation": "prove",
  "stage": "initial",
  "phase": {
    "kind": "combined",
    "components": ["execution", "proving"]
  }
}
```

zkperf MUST NOT synthesize individual execution or proving durations from
that combined boundary.

The v1 operation-to-phase mappings are closed:

| Operation and stage | Allowed phase |
| --- | --- |
| `prepare/build` | `build` or `combined:build+setup` |
| `prepare/setup` | `setup` |
| `execute` | `execution` |
| `prove/initial` | `proving` or `combined:execution+proving` |
| `prove/transform` | `compression` |
| `verify` | `verification` |

Any other mapping is a malformed capability response. This prevents an
adapter from causing zkperf to classify, for example, verification work as a
build measurement.

Every supported measurable operation or stage MUST have exactly one boundary.
Unsupported or unadvertised operations and stages MUST NOT have boundaries.
Every request for an unadvertised operation, stage, proof mode, or
transformation MUST return `unsupported`; returning `success` or `error` is a
capability mismatch.

### 7.2 `metadata`

`metadata` reports stable adapter and engine identity plus the resolved SDK,
toolchain, proof system, backend, verifier, and engine configuration. The
request MAY include prepared artifacts when metadata is available only after
build or setup.

The returned `configuration` is non-secret JSON. Unknown engine-specific
metadata belongs in a namespaced `extensions` object. Metadata MUST describe
the resolved runtime, not merely repeat requested values.

### 7.3 `prepare`

`prepare` has three stages:

| Stage | Fairness treatment |
| --- | --- |
| `environment` | Untimed preparation: dependency checks, installation, or acquisition of pinned public parameters. |
| `build` | Timed production of the engine-specific guest artifact. |
| `setup` | Timed creation of reusable program-specific state, keys, or parameters. |

The request includes benchmark identity, adapter configuration, input
artifacts, and cache state where applicable. The result identifies every
prepared artifact that later operations may consume.

An adapter MUST NOT hide input-dependent execution or proving inside `setup`.
If the engine cannot separate a stage, it advertises the actual combined
boundary and returns `unsupported` when zkperf requests the unavailable
individual stage.

### 7.4 `execute`

`execute` consumes exactly one canonical input artifact plus any prepared
artifacts. A success result identifies:

- the canonical output artifact;
- the complete execution result or trace required by the prover;
- observed commitment digests, when applicable; and
- optional scalar observations such as engine cycle count.

The canonical output bytes MUST match the workload specification. An engine
journal or receipt is not a substitute for the decoded canonical output.
Incorrect output is an `error` in the `execution` phase.

### 7.5 `prove`

`prove` has two stages:

| Stage | Meaning |
| --- | --- |
| `initial` | Create the first independently verifiable proof from a completed execution result. It maps to `proving`. |
| `transform` | Apply one declared recursion, wrapping, aggregation, or compression transformation to a completed proof. It maps to `compression`. |

An `initial` request names a proof mode. A `transform` request additionally
names a `transformation_id` advertised for that mode. The result identifies
the proof artifact, its public-values artifact when present, and its observed
commitments.

A successful initial proof artifact uses the selected proof mode's
`proof_format` as its exact `media_type`. A successful transformed proof uses
the selected transformation's `output_format`. Both capability fields are
media types, not display labels.

A proof mode identifier is adapter-scoped and stable for one adapter MAJOR
version. The mode also declares its proof format, proof system, backend,
features, and ordered transformations. zkperf compares the declared semantic
properties, never display labels or identifier spelling across adapters.
Proof mode identifiers MUST be unique within one capability response.
Transformation identifiers MUST be unique within their containing proof mode.
An adapter that supports `prove` MUST advertise at least one proof mode.

### 7.6 `verify`

`verify` consumes a proof, the public statement, expected output digest, and
expected commitment digests. Success means the verifier accepted the complete
statement. Its result repeats the accepted verdict and observed digests.

Verifier rejection, incomplete commitment checks, wrong public output, and
mocked or skipped verification are `error` responses in the `verification`
phase. A rejected proof MUST NOT be returned as a successful operation with
`verdict: "rejected"`.

## 8. Optional capabilities and proof modes

An optional operation, stage, proof mode, or transformation is represented in
both discovery and execution:

- capabilities mark support explicitly and enumerate stages and modes;
- an unadvertised optional request receives `status: "unsupported"`;
- `unsupported` includes a stable error code and the phase that could not be
  observed; and
- zkperf records the requested measurement as unavailable rather than zero or
  failed.

An adapter MUST NOT claim support based only on a best-effort runtime guess.
If an advertised capability depends on configuration, the adapter may return
a configuration-specific structured `error` or `unsupported` response. This
does not permit an unadvertised request to return `error`.

The ordered `transformations` array is the proof pipeline. Different order,
parameters, proof formats, backends, or verifier identities are different
proof modes for comparability purposes.

## 9. Timing

The host clock is authoritative for benchmark duration. An adapter MUST NOT
return a duration that zkperf uses as canonical phase time.

For a subprocess operation, the host starts timing immediately before process
creation and stops only after:

1. the adapter process exits;
2. inherited stdout and stderr handles close; and
3. every required output artifact is readable.

The boundary advertised by `capabilities` determines which fairness phase that
duration represents. Setup of the operation workspace before process creation
and artifact hashing after artifacts become readable are harness overhead and
are excluded from the component phase. An `end_to_end` aggregate includes
inter-operation orchestration as required by the fairness contract.

Adapter-provided scalar observations such as engine cycles MAY appear in
operation results. They are engine observations, not substitutes for the host
duration.

## 10. Timeout and cancellation

`timeout.limit_ns` is fixed by the benchmark plan. The host watchdog is
authoritative and begins and ends at the operation timing boundaries.

For graceful cancellation, the host atomically creates
`control_dir/cancel.json`:

```json
{
  "protocol": "zkperf-adapter",
  "protocol_version": "1.0.0",
  "request_id": "10000000-0000-4000-8000-000000000006",
  "reason": "user_cancelled"
}
```

An adapter advertising `control_file` cancellation MUST poll this file while
performing cancellable work, stop starting new work after observing it, clean
up temporary output, terminate its child processes, and emit an `error`
response with code `cancelled` and its current failure phase when it can do so
safely.

At a timeout, zkperf creates the same file with reason `deadline_exceeded`.
The benchmark outcome remains `timed_out` even if the adapter returns a
structured cancellation error during grace. Grace time is recorded separately
and MUST NOT be added to the timed duration.

After `termination_grace_ns`, or immediately when graceful cancellation is not
advertised, zkperf terminates the complete adapter process tree using the
platform's supervised-process mechanism. No descendant may survive. Partial
artifacts are retained only as diagnostic evidence and MUST NOT be accepted as
successful outputs.

User cancellation is a harness cancellation, not an engine benchmark failure.
The run model decides whether the containing report is retained as `invalid`
or unfinished; it MUST NOT relabel the cancellation as a successful,
unsupported, or timed-out sample.

## 11. Failure model

### 11.1 Adapter failure phases

Every adapter `error` or `unsupported` response names exactly one phase:

| Phase | Meaning |
| --- | --- |
| `protocol` | Envelope, version, parameter, or request semantic validation. |
| `capabilities` | Runtime capability discovery. |
| `metadata` | Runtime identity or configuration inspection. |
| `preparation` | Untimed environment preparation. |
| `build` | Guest build. |
| `setup` | Reusable program-specific setup. |
| `execution` | Canonical workload execution or output validation. |
| `proving` | Initial independently verifiable proof generation. |
| `compression` | Optional proof transformation. |
| `verification` | Complete statement verification. |
| `artifact` | Adapter-side artifact read, write, hash, or materialization. |
| `cleanup` | Adapter cleanup after otherwise completed work. |

The phase is the stage in which the failure occurred, not merely the requested
operation. For example, a `prove` transform failure uses `compression`, while
an unreadable execution trace detected by the prover uses `artifact`.

The operation-specific phase is fixed when the failure is not cross-cutting:

| Request | Required failure phase |
| --- | --- |
| `capabilities` | `capabilities` |
| `metadata` | `metadata` |
| `prepare/environment` | `preparation` |
| `prepare/build` | `build` |
| `prepare/setup` | `setup` |
| `execute` | `execution` |
| `prove/initial` | `proving` |
| `prove/transform` | `compression` |
| `verify` | `verification` |

`protocol`, `artifact`, and `cleanup` are cross-cutting and may replace the
operation-specific phase only when the failure actually occurred in that
cross-cutting work.

### 11.2 Host failure phases

Failures that prevent or invalidate an adapter response are classified by the
host:

| Host phase | Examples |
| --- | --- |
| `discovery` | Missing, duplicate, or invalid adapter manifest. |
| `negotiation` | No common version or manifest/runtime capability mismatch. |
| `launch` | Executable missing, permission denied, or process creation failed. |
| `transport` | stdin write failure, malformed stdout, response mismatch, or non-zero exit. |
| `timeout` | Watchdog deadline expired. |
| `cancellation` | User or supervisor cancelled the operation. |
| `artifact_validation` | Escaping path, wrong digest, wrong length, or unreadable output. |
| `cleanup` | Process-tree or workspace cleanup failed. |

Host and adapter phases are retained together when both exist. A process may,
for example, report an adapter `proving` error and then fail host cleanup.
Neither observation erases the other.

### 11.3 Malformed requests and responses

When an adapter can decode the envelope and correlate the request, invalid
operation parameters produce a schema-valid `error` response with phase
`protocol`. When it cannot determine the protocol version, request ID, or
operation, it exits `64`, writes stderr diagnostics, and emits no stdout.

zkperf classifies any of the following as a malformed response:

- empty stdout;
- invalid UTF-8 or JSON;
- a top-level value other than one object;
- more than one JSON value or any non-whitespace framing bytes;
- a schema violation or unknown normative property;
- a missing, duplicate, or unresolved artifact identifier;
- a response larger than the negotiated maximum; or
- an output artifact that changes while being validated.

A valid but mismatched `request_id`, `operation`, or `protocol_version` is a
`response_mismatch`, which is also a host transport failure. Malformed or
mismatched responses are never partially accepted. They remain evidence and
are not retried unless the benchmark policy explicitly permits replacement
for a verified harness failure.

### 11.4 Normalization into BenchmarkReport v1

Adapter protocol data is intermediate evidence, as required by
[`benchmark-report-v1.md`](benchmark-report-v1.md). The host validates the
transaction before normalizing it into the fairness-contract statuses and the
BenchmarkReport v1 model:

| Protocol or host outcome | Benchmark sample treatment |
| --- | --- |
| Validated adapter `success` | `success`, after all required correctness checks pass |
| Adapter `unsupported` | `unsupported`, or an unavailable requested measurement |
| Adapter `error` | `failed` |
| Host watchdog expiry | `timed_out`, even if the adapter reports cancellation during grace |
| External interference or harness invalidation | `invalid` |

An adapter success never overrides a host transport, artifact-validation, or
correctness failure. Only a normalized `success` duration is statistically
eligible.

Protocol measurement boundaries use exactly the report component names
`build`, `setup`, `execution`, `proving`, `compression`, and `verification`.
`prepare/environment` remains untimed preparation. `end_to_end` is a
host-supervised aggregate and is never advertised as an adapter operation
boundary.

Protocol artifacts are not copied blindly into report artifacts. When the
host retains one, `proof`, `proving_key`, and `verification_key` keep their
report kinds; `execution_trace` becomes `trace`; `profile` remains `profile`;
and protocol kinds without a dedicated report kind become `other`.
Canonical output and commitment digests populate phase-applicable report
correctness fields after validation.

## 12. Determinism and security

- Protocol JSON MUST NOT contain credentials, private input bytes, signing
  material, or other secrets. Private inputs remain file-backed artifacts with
  the minimum required filesystem permissions.
- Extensions are allowed only in a namespaced `extensions` object. An
  extension MUST NOT change the meaning of a normative field.
- Unknown normative properties are invalid in v1.0.0. Unknown extension keys
  are ignored by consumers that do not understand them and preserved by
  lossless transformations.
- Object key order is insignificant. Array order is significant for commands,
  phase components, proof transformations, and any field documented as
  ordered.
- Adapters SHOULD produce deterministic JSON for identical metadata and
  capability requests. Engine nondeterminism MUST be disclosed in metadata
  and must follow the workload and fairness contracts.
- The host MUST bound stdout, stderr capture, artifact count, individual
  artifact size, total output size, process count, and execution time.

## 13. Conformance

A conforming adapter protocol implementation MUST demonstrate:

1. manifest and runtime version negotiation;
2. schema-valid requests and responses for every supported operation;
3. explicit unsupported behavior for optional stages and proof modes;
4. exact response correlation;
5. stdout isolation under noisy SDK and child-process output;
6. content, path, length, and digest validation for every artifact;
7. phase-specific structured failures;
8. timeout, graceful cancellation, forced process-tree termination, and
   partial-artifact behavior;
9. rejection of malformed requests and responses; and
10. preservation of stderr and protocol evidence for unsuccessful operations.

The checked-in fixture contains successful exchanges for all six operations,
separate initial proving and compression, plus unsupported, failure, and
cancellation responses. Run the repository conformance tests with:

```sh
python -m unittest discover -s tests -v
python tools/validate_adapter_protocol.py examples/protocol-v1.json
```

The schema validates message shape. The protocol validator additionally checks
request/response correlation, prepare and prove result identity, verification
digests, response-wide artifact ID uniqueness, proof mode and transformation
ID uniqueness, artifact identifier resolution and workspace ownership,
operation-specific artifact kinds, capabilities-first sequencing, run-wide
request ID uniqueness, highest-exact selected-version agreement, capability
and boundary completeness for every response status, failure-phase attribution,
manifest/runtime agreement, and canonical output digests. Runtime
implementations still need filesystem digest validation, process sequencing,
and the fairness-contract mapping described above.
