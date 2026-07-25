# zkperf BenchmarkReport v1

| Field | Value |
| --- | --- |
| Model ID | `zkperf-benchmark-report` |
| Model version | `1.0.0` |
| JSON Schema | [`../schemas/benchmark-report-v1.schema.json`](../schemas/benchmark-report-v1.schema.json) |
| Fairness contract | [`benchmark-fairness-contract-v1.md`](benchmark-fairness-contract-v1.md) |
| Source issue | `Dnreikronos/zkperf#2` |

`BenchmarkReport` is the immutable result model shared by collectors,
comparisons, and every renderer. It contains the raw observations and the
derived summaries for one benchmark case executed by one engine in one run.
Engine adapters may produce intermediate data, but that data is not a zkperf
result until it has been normalized into this model.

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT,
RECOMMENDED, MAY, and OPTIONAL are to be interpreted as described in RFC 2119
and RFC 8174 when they appear in uppercase.

## 1. Immutability and ownership

A report is complete and immutable once emitted.

- `report_id` and `run.id` MUST remain stable for the lifetime of the report.
- A producer MUST finish all samples, summaries, artifacts, warnings, and the
  final report status before publishing the report.
- A correction, retry, or re-analysis MUST create a new report with a new
  `report_id`. It MUST NOT overwrite a published report.
- Derived formats MUST consume the report. They MUST NOT query an engine,
  re-read engine-specific output, or maintain a second result model.
- Comparisons MUST use the canonical fields in the report and the rules in the
  fairness contract. Display strings are never comparison keys.

The JSON document is the lossless representation. Terminal, CSV, and HTML
outputs are projections and may omit detail for presentation, but MUST retain a
link or identifier for the source report.

## 2. Top-level model

Every report contains these required sections:

| Field | Meaning |
| --- | --- |
| `schema_version` | Exact version of this model. v1 documents use `1.0.0`. |
| `report_id` | Globally unique identity for this immutable report. |
| `contract` | Fairness contract ID and version governing the result. |
| `run` | Run identity, suite, policy, and complete planned schedule. |
| `benchmark` | Canonical case, workload, input, statement, and lifecycle metadata. |
| `engine` | Engine, adapter, guest, proof-system, verifier, and configuration metadata. |
| `environment` | Host, resource profile, and monotonic measurement clock. |
| `measurements` | Requested metrics, raw samples, and derived statistics. |
| `artifacts` | Content-addressed outputs associated with the report. |
| `warnings` | Non-fatal conditions that consumers must be able to display. |
| `status` | Final report outcome. |

`extensions` is the only extension point. Extension keys MUST be namespaced,
for example `org.example.gpu_counters`. Extensions MUST NOT redefine a
normative field or alter its meaning.

Fields that do not apply MUST be absent. Producers MUST NOT use `null`, an
empty string, zero, or an empty object as a missing-value sentinel.

## 3. Run identity and policy

`run` binds the report to a harness revision, suite revision, manifest digest,
wall-clock audit interval, declared policy, and planned attempt order.
Wall-clock timestamps are for audit only. They MUST NOT be used to calculate
benchmark durations.

The planned schedule MUST be saved before the first attempt. Every raw sample's
`schedule_position`, `attempt_index`, `warmup`, and `phase` MUST match one
planned entry. A sample may be present in several metrics through its shared
`attempt_id`; its metric-local `id` remains unique.

`run.policy` records ordering, concurrency, retry, invalidation, outlier, and
percentile choices. Metric-specific warm-up count, measured-trial count, and
timeout policy live on every requested duration measurement, including an
unavailable one, because the fairness contract fixes them per case and phase.

## 4. Benchmark and engine identity

`benchmark` contains the metadata that defines semantic equivalence:

- case and workload revisions;
- workload and canonical input digests;
- canonical input length and visibility;
- preprocessing, statement, and input/output commitment policies;
- implementation lane and security target;
- expected output digest;
- lifecycle and cache state; and
- any case parameters needed to reproduce the run.

`engine` records the implementation under test. Its configuration may differ
from another engine without changing benchmark identity, but all differences
required by the fairness contract remain explicit. The configured security
level MUST meet or exceed `benchmark.security_target_bits`.

`engine.proof` records verifier and parameter versions as well as the parameter
digest. Every successful proof-producing attempt MUST also have exactly one
`proof_size` observation in the same phase, using canonical bytes.

Configuration and parameter objects MUST contain non-secret, JSON-compatible
values. Tokens, private inputs, credentials, and other secrets MUST NOT appear
in a report.

## 5. Environment

`environment.host` identifies the machine, CPU topology, RAM, accelerators,
storage, operating system, and available firmware or microcode metadata.
`environment.resources` fixes the power, CPU, memory, worker, accelerator,
environment-variable, network, and local/remote profile.

A local measured profile MUST set `network_access` to `false`. A remote profile
MUST include the non-secret endpoint identity, regions, transport, connection
type, and observed network characteristics required by the fairness contract.

`environment.clock` MUST describe the monotonic clock used by every duration
sample. Its resolution is an integer number of nanoseconds.

## 6. Measurements and canonical units

Each requested metric appears exactly once for a given phase. Producers MUST
reject duplicate `(metric, phase)` entries.

The v1 metric catalog and canonical units are:

| Metric | Canonical unit | Raw value |
| --- | --- | --- |
| `duration` | `nanosecond` | Successful timing tuple |
| `peak_resident_memory` | `byte` | Non-negative integer |
| `proof_size` | `byte` | Non-negative integer |
| `proving_key_size` | `byte` | Non-negative integer |
| `verification_key_size` | `byte` | Non-negative integer |
| `constraint_count` | `count` | Non-negative integer |
| `cycle_count` | `count` | Non-negative integer |

Units are singular, case-sensitive identifiers. Producers MUST convert values
to canonical units before constructing the report. Renderers may show a scaled
value such as milliseconds or MiB, but the scaling is presentation-only and
the canonical value MUST remain available.

New standard metrics require a new schema version. Engine-specific metrics
belong in a namespaced `extensions` object until standardized.

### 6.1 Available measurements

An available measurement has:

- `availability: "available"`;
- all raw `samples`, including warm-ups and unsuccessful attempts; and
- `statistics`, either an available summary or an unavailable summary with a
  reason.

Duration measurements also declare the phase, observable start and stop
boundaries, interface type, and policy. The policy contains warm-up and
measured-trial counts plus a timeout limit, enforcement mechanism, and
termination grace period. Combined phases use an ordered `components` array.
They are distinct from every individual component and from `end_to_end`.

### 6.2 Unavailable measurements

An engine that cannot expose or perform a requested metric emits:

```json
{
  "metric": "duration",
  "unit": "nanosecond",
  "phase": {
    "kind": "standard",
    "name": "execution"
  },
  "availability": "unavailable",
  "reason": {
    "code": "inseparable_phase",
    "message": "The engine only exposes execution and proving as one operation."
  },
  "policy": {
    "warmup_count": 0,
    "measured_trial_count": 10,
    "timeout": {
      "limit_ns": 60000000000,
      "enforcement": "host_watchdog",
      "termination_grace_ns": 1000000000
    }
  }
}
```

An unavailable measurement MUST contain a stable, machine-readable reason code
and a human-readable message. It MUST NOT contain samples or statistics.
Unavailable values MUST never be encoded as zero.

An unavailable duration measurement still contains `policy`. Unsupported work
does not erase the suite's declared warm-up count, measured-trial count,
timeout limit, enforcement mechanism, or termination grace period. Comparison
uses those values for `C09_SCHEDULE` and disclosure uses them for
`C12_DISCLOSURE`.

## 7. Raw samples

Raw samples are append-only observations and MUST never be deleted. A sample
contains a unique sample ID, shared attempt ID, attempt and schedule indexes,
warm-up flag, audit timestamps, and outcome.

Duration sample statuses follow the fairness contract:

| Status | Required data | Statistical eligibility |
| --- | --- | --- |
| `success` | Timing plus phase-applicable correctness | Eligible when not a warm-up and not invalidated |
| `unsupported` | Non-empty reason | Never |
| `failed` | Stable error code and non-empty reason | Never |
| `timed_out` | Outcome; optional diagnostic timing and reason | Never |
| `invalid` | Non-empty reason | Never |

For every successful duration:

```text
stop_ns >= start_ns
duration_ns = stop_ns - start_ns
```

All three values come from `environment.clock`. Diagnostic timing on an
unsuccessful attempt uses `diagnostic_timing`; it MUST NOT enter performance
statistics.

Correctness is phase-specific:

- `build` and `setup` successes omit `correctness`;
- `execution` success includes the expected output digest;
- `proving`, `compression`, `verification`, and `end_to_end` success includes
  the expected output digest, an `accepted` verification verdict, and every
  commitment digest required by `benchmark.commitment_policy`; and
- a combined phase follows the strongest applicable component rule.

`rejected` is never a successful verdict. A rejected or incomplete proof makes
the attempt `failed`. A failed proof MAY retain `correctness` with any observed
digests and `verification_verdict: "rejected"` so the raw rejection evidence
is not lost.

Scalar samples use `value` only for `success`. Byte and count observations are
non-negative integers. A scalar metric that is unsupported for the entire
report should be an unavailable measurement rather than a set of unsupported
samples.

`replacement_attempt_id` is present only on an allowed replacement for an
invalid attempt. It points to the shared `attempt_id`, never a metric-local
sample ID. Every metric sample for that replacement attempt MUST carry the same
reference.

## 8. Statistics

Statistics are derived, never substituted for raw data.

An available summary MUST include:

- the successful measured sample count;
- minimum, maximum, and median;
- percentile values calculated with `run.policy.percentile_method`;
- exact attempt IDs included in summaries, excluded as warm-ups, and flagged
  as outliers;
- counts for every sample status; and
- failure and timeout rates over non-warm-up scheduled attempts.

`included_attempt_ids` MUST contain every valid, successful, non-warm-up
attempt used by the primary summary and no other attempt. Primary summaries
MUST NOT exclude statistical outliers. `flagged_outlier_attempt_ids` is
diagnostic only.

`run.policy.percentile_method` is the sole source of truth for percentile
calculation. Statistics MUST NOT repeat or override it.

The summary values use the measurement's canonical unit. Averages and
percentiles may therefore be non-integers even when every raw observation is an
integer.

When there are no eligible successful samples, `statistics.status` is
`unavailable` and `reason` explains why. Counts and rates remain present.
Producers MUST NOT emit a zero-valued summary for an empty population.

The JSON Schema validates shape and ranges. Producers and conformance tests
MUST additionally validate arithmetic and cross-reference invariants such as:

- timing subtraction;
- sample, status, and schedule counts;
- summary values recalculated from included attempt IDs;
- rates recalculated from measured attempts;
- ID references;
- replacement lineage shared across metric samples;
- phase-applicable correctness data;
- one proof-size observation for every successful proof-producing attempt;
- unique `(metric, phase)` keys; and
- report status consistency.

## 9. Artifacts and warnings

Artifacts are content-addressed references, not embedded binary data. Each
artifact includes its kind, URI, media type, byte length, and SHA-256 digest.
`attempt_id` associates a per-attempt artifact with the shared attempt.
Artifact and warning attempt references MUST resolve to an attempt retained in
the report.

Warnings are non-fatal. Their stable code is intended for filtering and their
message is intended for people. `path` is a JSON Pointer to the affected
report field. Fatal conditions belong in sample or report status, not warnings.

## 10. Report status

The final `status.outcome` is one of:

| Outcome | Meaning |
| --- | --- |
| `successful` | All requested required measurements completed successfully. |
| `failed` | The run completed with a fatal engine, correctness, or harness failure. |
| `timed_out` | A declared timeout prevented completion. |
| `partially_supported` | The report is usable, but at least one requested metric is unavailable. |
| `invalid` | External interference or a harness failure invalidated the report. |

Every non-successful report status includes a reason. Individual failed or
timed-out samples remain in `measurements`; the top-level status is not a
replacement for raw outcomes.

## 11. Schema evolution and unknown fields

`schema_version` uses Semantic Versioning.

- MAJOR changes may remove fields or change existing meaning.
- MINOR changes may add optional fields, metrics, enum values, or extension
  points without changing existing meaning.
- PATCH changes clarify constraints without changing valid data semantics.

Published schemas and reports are immutable. Each model version gets its own
schema asset. The v1.0.0 schema is closed: unknown normative properties are
invalid, which catches misspellings and accidental data loss. Namespaced data
is accepted only inside `extensions`.

A consumer MUST select a parser by `schema_version` before interpreting the
document:

1. An unsupported MAJOR version MUST be rejected.
2. A consumer that explicitly supports a newer compatible v1 MINOR version MUST
   ignore unknown optional properties and preserve them when round-tripping.
3. Unknown enum values MUST NOT be coerced to a known value. The consumer MUST
   reject the affected operation or expose it as unsupported.
4. Unknown extension keys MUST be ignored by consumers that do not understand
   them and preserved by lossless transformations.

Validation against the exact declared schema happens before comparison or
rendering.

## 12. Renderer and comparison contract

All zkperf renderers MUST accept a validated `BenchmarkReport` and nothing
engine-specific.

- JSON rendering is lossless.
- Terminal and HTML renderers show unavailable reasons, warnings, report
  status, sample counts, failure rates, timeout rates, and canonical units.
- CSV renderers use empty value cells plus explicit availability and reason
  columns. They MUST NOT write zero for unavailable data.
- A renderer MAY scale values, but MUST label the display unit and preserve the
  canonical unit in machine-readable output.
- Comparison code uses raw samples and canonical summaries from reports. It
  MUST return the boolean decision plus every failed `C01` through `C12` rule
  from the fairness contract.
- Ratios and ranks are forbidden when comparability fails or either summary is
  unavailable.

## 13. Examples

The checked-in examples are conformance fixtures:

- [`successful.json`](../examples/reports/successful.json)
- [`failed.json`](../examples/reports/failed.json)
- [`timed-out.json`](../examples/reports/timed-out.json)
- [`partially-supported.json`](../examples/reports/partially-supported.json)

Every fixture MUST validate against the v1 JSON Schema.

Validation tooling requires Python 3.10 or newer.

Run repeatable schema and semantic conformance checks with:

```sh
python -m pip install -r requirements-dev.txt
python -m unittest discover -s tests -v
python tools/validate_benchmark_reports.py examples/reports/*.json
```

The test suite validates the schema itself, all positive fixtures, checked-in
negative cases, timing arithmetic, schedule and ID references, summaries,
replacement lineage, phase correctness, and required per-attempt proof sizes.
