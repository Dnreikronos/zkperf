# zkperf Benchmark Fairness and Reproducibility Contract

| Field | Value |
| --- | --- |
| Contract ID | `zkperf-fairness` |
| Contract version | `1.0.0` |
| Status | Draft |
| Source issue | `Dnreikronos/zkperf#1` |

This contract defines when zkVM benchmark results produced by zkperf are fair,
reproducible, and comparable. A result that does not satisfy every applicable
requirement in this document MUST NOT be presented as conforming to this
contract.

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT,
RECOMMENDED, MAY, and OPTIONAL are to be interpreted as described in RFC 2119
and RFC 8174 when they appear in uppercase.

## 1. Versioning

Every benchmark record MUST include the exact `contract_id` and
`contract_version`.

- A MAJOR change may alter the meaning of an existing field, timing boundary, or
  comparability decision.
- A MINOR change may add a backward-compatible requirement or measurement.
- A PATCH change may clarify wording without changing a result.

Published contract versions are immutable. Corrections MUST be released as a
new version. Results governed by different MAJOR versions are not comparable.
Results governed by different MINOR or PATCH versions are comparable only when
the newer contract explicitly declares backward compatibility.

## 2. Benchmark unit

The unit of comparison is a **benchmark case**. A case is the tuple:

1. workload specification and revision;
2. implementation lane;
3. canonical input bytes;
4. application-level statement and committed-output policy;
5. security target;
6. lifecycle and resource profile; and
7. measurement phase.

An **engine adapter** translates this case into the APIs or commands of one
zkVM engine. Adapter work MAY differ only where this contract permits it.

An **attempt** is every scheduled invocation, including warm-ups and
unsuccessful invocations. A **measured trial** is a non-warm-up attempt whose
status and any applicable duration are retained in the result set.

## 3. Semantic equivalence

### 3.1 Workload specification

Each case MUST have a version-controlled workload specification. The
specification MUST define:

- the input schema and canonical encoding;
- whether each input is public or private;
- the computation and allowed numeric behavior;
- the expected application-level output;
- which input and output bytes are committed by the proof;
- accepted termination conditions; and
- any permitted nondeterminism.

The record MUST contain a collision-resistant digest of the workload
specification. zkperf uses lowercase, hexadecimal SHA-256 digests unless a
suite version explicitly chooses another algorithm.

### 3.2 Inputs

All engines in one comparison MUST consume the same canonical input byte
stream. The record MUST contain its byte length and digest.

Serialization performed by an adapter is allowed only when it is a lossless
transport encoding. The bytes observed by the guest after decoding MUST equal
the canonical input. Padding, truncation, alternate precision, pre-hashing, or
precomputation outside the guest makes results non-comparable unless that
transformation is part of the workload specification for every engine.

Input visibility MUST match the workload specification. A result using public
input is not comparable with one using private input.

### 3.3 Computation

Every guest MUST implement the same application-level computation and
termination behavior. A suite MUST assign each implementation to one of these
lanes:

| Lane | Rule |
| --- | --- |
| `portable` | The algorithm, data representation, and relevant operation sequence are equivalent. Engine-specific acceleration and precompiles are forbidden. |
| `optimized` | Engine-specific libraries, accelerators, precompiles, and algorithms are allowed, but the input/output semantics and proof statement MUST remain equivalent. |

Results from different lanes are not comparable. Optimized implementations
MUST identify every engine-specific optimization in metadata. Moving work from
the guest to unproved host code is forbidden in both lanes unless the workload
specification requires the same host preprocessing for every engine and the
proof commits to its result.

The guest source revision, toolchain, compiler flags, and final guest artifact
digest MUST be recorded. Guest artifact digests are expected to differ across
engines and are not, by themselves, evidence of semantic inequivalence.

### 3.4 Committed output and statement

The workload specification MUST define one canonical application output byte
stream. Build and reusable setup are not trial phases and have no application
output, commitment, or verification obligation.

A successful `execution` phase MUST produce bytes equal to the expected
application output. A successful proof-producing phase (`proving` or
`compression`) MUST:

1. preserve bytes equal to the expected application output;
2. commit to the input and output bytes required by the workload
   specification; and
3. expose enough public data for the verifier to check the complete
   application-level statement.

A successful `end_to_end` phase MUST satisfy the execution and proof-producing
requirements above and the verification requirements in Section 3.5.

Adapters MAY wrap the canonical output in an engine-specific journal, receipt,
or public-values format. The adapter MUST decode that format and demonstrate
that the committed canonical bytes are unchanged. Comparing a proof that binds
an input or output with one that does not is forbidden.

### 3.5 Verification

Verification MUST use the verifier and verification parameters associated
with the proof-producing engine and MUST check the complete statement required
by the workload specification. A successful `verification` phase MUST return
an accepting verdict for that statement. A successful `end_to_end` phase MUST
include the same complete verification.

An untimed verification used to establish correctness for `proving` or
`compression` occurs after that phase's stop boundary and MUST NOT be included
in its duration. Missing, skipped, mocked, or partial verification cannot
establish correctness.

The configured security target in bits MUST be the same for all engines in a
comparison. Each engine configuration MUST meet or exceed it. Proof-system,
curve, field, hash, verifier, and trusted-setup differences MUST be reported;
they are allowed only when they satisfy the same declared security target and
trust profile.

## 4. Measurement phases

### 4.1 Common timing rules

Durations MUST use a monotonic, high-resolution host clock and MUST be stored
as integer nanoseconds. Wall-clock timestamps MAY be recorded for audit
purposes but MUST NOT be used to calculate durations.

Every record that reports a duration MUST store `start_ns`, `stop_ns`, and
`duration_ns` from the same clock, with
`duration_ns = stop_ns - start_ns`. Diagnostic elapsed time for an
unsuccessful attempt MUST be labeled as diagnostic and MUST NOT enter
performance statistics.

For an in-process API, the start boundary is immediately before entering the
named engine or adapter operation and the stop boundary is immediately after it
returns. For a subprocess, the start boundary is immediately before process
creation and the stop boundary is after the process exits and all required
output artifacts are readable. Startup and teardown inside those boundaries
are included.

If an engine operation is asynchronous, the stop boundary occurs only after an
explicit synchronization confirms completion. Lazy work triggered after a
return MUST be synchronized and included in the phase that caused it.

Phases MUST NOT overlap. Host orchestration between phase boundaries is
excluded from individual phase durations and included in `end_to_end`.

### 4.2 Preparation

Preparation includes installing toolchains, downloading dependencies or public
parameters, provisioning the host, and obtaining source code. It is not a
timed phase. Every preparation input MUST still be pinned and recorded.

Network access MUST be disabled during measured phases. A suite that
intentionally measures remote proving MUST define a separate resource profile
and MUST NOT compare it with local proving.

### 4.3 Build

`build` measures production of the engine-specific executable guest artifact.

- **Start:** immediately before invoking the declared build command with the
  declared cache state.
- **Stop:** after the build command exits successfully and all guest artifacts
  required by setup are readable.

The record MUST declare the cache state as `cold`, `warm`, or `incremental`.
Cold build comparisons MUST begin from equivalent empty disposable build
caches. Warm and incremental build results are separate profiles and MUST NOT
be compared with cold builds.

Dependency download and toolchain installation belong to preparation, not
build. A build that downloads undeclared dependencies is invalid.

### 4.4 Setup

`setup` measures reusable, program-specific engine preparation after build,
including program loading, program commitment, circuit synthesis, key
generation, and proving or verification parameter derivation when applicable.

- **Start:** immediately before the first engine operation that consumes the
  built guest artifact to create reusable program-specific state.
- **Stop:** after every declared reusable setup artifact is fully materialized
  and before any trial input is submitted.

Setup MUST declare its cache state and reuse count. A one-time setup duration
MUST NOT be silently amortized into trial durations. If setup is
input-dependent, it is not reusable setup and MUST remain in `execution`,
`proving`, or the applicable combined phase.

### 4.5 Execution

`execution` measures production of the complete guest execution result needed
by the prover.

- **Start:** immediately before submitting the canonical trial input to the
  engine's execution operation.
- **Stop:** after the guest has terminated and its output plus complete
  execution result or trace is materialized and synchronized.

Proof generation, recursion, compression, and verification are excluded.
Input serialization before submission is excluded and MUST be deterministic;
engine-side decoding after submission is included.

### 4.6 Proving

`proving` measures creation of the engine's first independently verifiable
proof artifact from an already completed execution result.

- **Start:** immediately before invoking the prover with the execution result
  and proving parameters ready.
- **Stop:** after the uncompressed proof artifact and its public statement are
  fully materialized and synchronized.

Proof serialization performed by the prover is included. Optional recursion,
wrapping, aggregation, or compression after the first independently verifiable
artifact is excluded and belongs to `compression`.

### 4.7 Compression

`compression` measures an optional transformation of a completed proof into
the final proof format selected by the suite. It includes recursion, wrapping,
aggregation, or size-reduction stages selected by that profile.

- **Start:** immediately before the transformation consumes the completed
  proof.
- **Stop:** after the selected final proof artifact and public statement are
  fully materialized and synchronized.

The exact ordered transformation pipeline MUST be recorded. Different
pipelines are not comparable.

### 4.8 Verification

`verification` measures a complete verifier decision for the selected proof
format.

- **Start:** immediately before invoking the verifier with the proof,
  statement, verification parameters, and canonical expected commitments
  ready.
- **Stop:** immediately after the verifier returns an accept or reject verdict
  and all asynchronous verifier work is synchronized.

Loading proof bytes or parameters from storage is excluded when they are
preloaded for all engines. If a suite includes I/O, it MUST include equivalent
I/O for every engine and identify the profile as `verification_with_io`.

### 4.9 End to end

`end_to_end` measures a single trial through verification.

- **Start:** immediately before submitting the canonical trial input to the
  first trial-specific engine operation.
- **Stop:** immediately after complete verification returns and synchronizes.

It includes execution, proving, the configured compression pipeline,
verification, and inter-phase adapter overhead. It excludes preparation,
build, and reusable setup.

### 4.10 Combined and unsupported phases

An adapter MUST NOT estimate, subtract, or invent a phase duration. When an
engine cannot expose separate boundaries, the adapter MUST record the smallest
observable ordered composition, such as `combined:execution+proving`, using the
start boundary of its first component and the stop boundary of its last
component.

A combined phase is comparable only with the same ordered combination. Its
component phases MUST have status `unsupported` and a non-empty reason.

Every requested phase MUST produce one of:

| Status | Meaning |
| --- | --- |
| `success` | Boundary was observable and the required correctness checks passed. |
| `unsupported` | The engine cannot expose or perform the phase; `reason` is REQUIRED. |
| `failed` | The engine returned an error or a correctness or verification check failed. |
| `timed_out` | The declared timeout expired. |
| `invalid` | External interference or harness failure invalidated the attempt; `reason` is REQUIRED. |

Only `success` has a performance duration eligible for summary statistics.
All statuses remain part of the reported attempt counts.

## 5. Trial policy

### 5.1 Policy declaration

Before execution, the suite manifest MUST fix, per case and phase:

- warm-up count;
- measured-trial count;
- engine ordering algorithm and random seed;
- concurrency mode and resource limits;
- timeout;
- retry and invalidation policy; and
- outlier rule and summary statistics.

There are no implicit defaults. A missing policy makes the run nonconforming.
The manifest digest MUST be stored with every result.

### 5.2 Warm-up

Every engine MUST receive the same number of warm-up attempts for a comparable
case. Warm-ups MUST use the same input and resource profile as measured trials.
They are marked `warmup: true`, retained for audit, and excluded from primary
statistics.

Warm-up state MAY be retained only when the lifecycle profile is `warm`.
For a `cold` profile, the declared caches and reusable state MUST be reset
before every measured trial.

### 5.3 Ordering

Measured trials SHOULD be interleaved by round so that every engine runs once
per round. Engine order MUST be derived from the recorded seed using the
declared algorithm. The resulting complete schedule MUST be saved before the
first measured trial.

Manual grouping, opportunistic reordering, or rescheduling based on observed
performance is forbidden. A changed schedule creates a different run.

### 5.4 Concurrency and host isolation

The concurrency mode MUST be one of:

- `isolated`: only one measured attempt runs on the host at a time; or
- `throughput`: the exact number and placement of concurrent attempts is fixed.

Isolated and throughput results are not comparable. Internal engine
parallelism is allowed, but CPU affinity, logical CPU limit, worker or thread
count, accelerator allocation, memory limit, and environment variables MUST be
fixed for the case. No other benchmark, build, update, or user workload may run
on the host during measurement.

### 5.5 Timeouts, failures, and retries

The same timeout MUST apply to all engines for a comparable case and phase. It
starts and stops at that phase's timing boundaries. Timeout enforcement and
termination grace time MUST be recorded; grace time is not part of the phase
duration.

Every scheduled attempt MUST be retained. Engine errors, invalid proofs,
incorrect output, verifier rejection, resource exhaustion, and timeouts are
results, not outliers, and MUST NOT be retried or replaced.

Only attempts invalidated by a documented harness failure or verified external
interference MAY be replaced. The invalid attempt remains in the dataset, its
reason is recorded, and its replacement points to the original attempt ID.

### 5.6 Outliers and statistics

Raw attempts MUST never be deleted. Primary latency statistics MUST include
every valid, successful measured trial. At minimum, report count, median,
minimum, maximum, and the interpolation method for every percentile.

An optional secondary analysis MAY flag statistical outliers only when the
rule was fixed in the suite manifest. Outlier-excluded numbers MUST be labeled
secondary and MUST be displayed beside, never instead of, the primary result.
Failure and timeout rates MUST accompany latency statistics.

Ratios or rankings MUST NOT be reported when either side lacks the manifest's
required number of valid successful trials.

## 6. Required metadata

The following metadata is REQUIRED for a conforming record.

| Category | Required fields |
| --- | --- |
| Contract | contract ID and version; harness revision; suite and manifest revision and digest |
| Case | case ID; workload revision and digest; implementation lane; canonical input length and digest; expected output digest; statement and commitment policy |
| Engine | engine name and version; adapter revision; proof system and backend; security target and configured security; trust or setup profile |
| Guest | source revision; toolchain and version; compiler and flags; artifact digest; engine-specific optimizations |
| Proof | raw and final proof format; transformation pipeline; proof byte length; verifier and parameter versions and digests |
| Host | machine identifier; CPU model and stepping; logical and physical CPU count; RAM; accelerator; storage; operating system and kernel; firmware or microcode where available |
| Resources | power profile; CPU affinity and limit; memory limit; worker or thread count; accelerator allocation; relevant environment variables; local or remote mode |
| Lifecycle | preparation pins; build and setup cache states; setup reuse count; warm-up count; measured-trial count |
| Schedule | concurrency mode; ordering algorithm; seed; complete planned order; attempt index and timestamps |
| Measurement | phase or ordered combined phase; interface type; exact boundary implementation; clock source; start, stop, and duration in nanoseconds; timeout |
| Outcome | status; phase-applicable output digest, commitment digests, and verification verdict; status-applicable error code and reason; replacement attempt ID when applicable |
| Statistics | included attempt IDs; excluded warm-up IDs; outlier rule and flagged IDs; percentile method |

Outcome fields are phase- and status-specific. A field that does not apply MUST
be absent rather than present with a null value.

- A `success` MUST include every output digest, commitment digest, and
  verification verdict required by the measured phase. It MUST omit error code
  and reason.
- An `unsupported` or `invalid` outcome MUST include the non-empty reason
  required by Section 4.10 and MAY include an engine or harness error code.
- A `failed` outcome MUST include a stable engine or harness error code and a
  non-empty reason.
- A `timed_out` outcome relies on the timeout in the Measurement metadata and
  MAY include an engine or harness error code and reason.
- `replacement_attempt_id` is REQUIRED only for a replacement attempt and MUST
  point to the invalid attempt it replaces. It MUST be absent otherwise.

Secrets and private input bytes MUST NOT be stored. Their digests, lengths, and
generation procedure are still required.

## 7. Comparability decision

Comparability is a deterministic pairwise decision for one measurement phase.
Given records `a` and `b`, the harness MUST return a boolean plus all failed
rule IDs. Records are comparable if and only if every rule below passes.

| Rule ID | Test |
| --- | --- |
| `C01_CONTRACT` | Contract IDs and versions are compatible under Section 1. |
| `C02_SUITE` | Suite revision, manifest digest, case ID, workload digest, and implementation lane are equal. |
| `C03_INPUT` | Canonical input digest and length, input visibility, and preprocessing policy are equal. |
| `C04_STATEMENT` | Expected output digest, statement, and input/output commitment policy are equal. |
| `C05_SECURITY` | Security target and trust profile are equal, and both configurations meet the target. |
| `C06_PHASE` | Phase name and boundary variant are equal; combined phases have the same ordered components. |
| `C07_LIFECYCLE` | Cache state, setup reuse, warm-up policy, and cold/warm profile are equal. |
| `C08_RESOURCES` | Host, operating-system, power, affinity, memory, thread, accelerator, storage, local/remote, and concurrency profiles are equal. |
| `C09_SCHEDULE` | Ordering algorithm, seed, planned schedule, measured-trial count, timeout, retry, invalidation, and outlier policies are equal. |
| `C10_SUPPORT` | Both engines mark the phase supported and have enough valid successful trials for the planned statistic. |
| `C11_CORRECTNESS` | Every included trial has the expected output and commitments and an accepting complete verification verdict where required. |
| `C12_DISCLOSURE` | All metadata required by Section 6 is present. |

Exact equality means canonical value equality, not display-string similarity.
The suite manifest MUST define canonical units and normalization for host and
resource fields.

Engine name and version, adapter revision, guest toolchain and artifact digest,
proof system, proof format, proof size, verifier implementation, and configured
security MAY differ across engines. These differences are the subject of the
benchmark and do not fail comparison when `C01` through `C12` pass. They MUST
still be disclosed.

### 7.1 Differences that make results non-comparable

The following differences always fail at least one comparability rule:

- workload, implementation lane, canonical input, input visibility, expected
  output, statement, or commitment policy;
- security target or trust profile, or an engine below the target;
- phase, combined-phase composition, boundary implementation, or local versus
  remote execution;
- cold, warm, or incremental cache state, or setup amortization;
- host hardware, firmware, operating system, power profile, resource limits,
  engine-visible parallelism, accelerator allocation, or concurrency mode;
- warm-up count, measured-trial count, order, seed, timeout, retry,
  invalidation, or outlier policy;
- compression, recursion, wrapping, or aggregation pipeline for compression
  and verification measurements;
- missing required metadata, insufficient successful trials, incorrect output,
  invalid commitment, or incomplete or rejecting verification; and
- incompatible contract or suite versions.

A report MAY show non-comparable results for diagnostic purposes, but MUST
label them `non-comparable`, list the failed rule IDs, and MUST NOT calculate a
speed ratio or rank between them.

## 8. Conformance assertions

Automated tests for every adapter and result writer MUST cover these assertions:

1. Changing any field named in `C01` through `C09` causes the expected rule to
   fail.
2. Engine-specific fields allowed after `C12` may differ without failing an
   otherwise valid comparison.
3. Canonical input and output fixtures round-trip through every adapter without
   byte changes.
4. Mutating committed input or output causes verification or the harness
   correctness check to fail.
5. An unavailable phase is emitted as `unsupported` with a reason and never as
   zero duration.
6. An inseparable phase is emitted as the exact ordered `combined:` phase and
   is not comparable with an individual component.
7. Failed, timed-out, invalid, and warm-up attempts are retained and excluded
   from latency statistics according to this contract.
8. Primary statistics contain every valid successful measured trial and no
   other attempt.
9. The saved schedule is reproducible from the declared algorithm and seed,
   and actual execution order matches it.
10. Every reported duration follows `stop_ns >= start_ns` and
    `duration_ns = stop_ns - start_ns`.
11. An asynchronous adapter cannot stop timing before explicit completion.
12. A record missing any required metadata fails `C12_DISCLOSURE`.

Passing these assertions demonstrates harness behavior, not workload semantic
equivalence. Each workload still requires reviewed fixtures and cross-engine
output and commitment tests.

## 9. Reporting requirements

A benchmark report MUST:

- identify this contract and the suite manifest;
- state whether each displayed comparison passed `C01` through `C12`;
- show unsupported phases, failures, timeouts, and invalid attempts;
- show primary statistics and the number of included attempts;
- disclose engine-specific proof, security, optimization, and setup choices;
- link or embed machine-readable raw records and the planned schedule; and
- avoid ratios and rankings for non-comparable records.

Reproduction instructions MUST be sufficient to provision the declared host
profile, obtain pinned sources and dependencies, rebuild artifacts, regenerate
the schedule from its seed, and run the same case without relying on mutable
external state.
