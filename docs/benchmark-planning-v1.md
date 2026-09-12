# Deterministic benchmark planning v1

Issue #9 expands the effective validated manifest into a complete schedule before
adapter discovery or execution. `BenchmarkPlan::build` takes ownership of that
manifest; the plan retains it as the single source of policy and configuration.

## Jobs and order

A job is one engine × workload × input × proof mode × repetition. It requests
the workload's ordered phase list; phases are not independent repetitions.
An engine with no proof modes receives one job with the proof mode absent
(the manifest permits this only for workloads without proof-related phases).
Each selected proof mode otherwise receives its own jobs.

`seeded_round_robin_v1` has the following fixed definition:

1. Rank engines by the lexicographic SHA-256 bytes of the UTF-8 string
   `zkperf-engine-order-v1`, followed by the seed as eight little-endian bytes,
   followed by the engine ID's UTF-8 bytes. Break ties by engine ID.
2. Emit all warm-ups, then all measured jobs. Within each group, visit workloads
   and their inputs in manifest declaration order.
3. For each workload/input, visit repetition indices starting at zero. Rotate
   the ranked engine list left by repetition modulo engine count. Visit each
   engine's proof modes in declaration order before moving to the next engine.
   Measured repetition indices and rotation restart at zero after warm-ups.
4. Assign a global zero-based position. `warmup` distinguishes excluded warm-ups
   from recorded repetitions; both remain in the plan.

Cardinality is `(warmups + runs) × sum(workload input counts) ×
sum(max(1, engine proof-mode counts))`. All arithmetic and vector reservation
are checked; unrepresentable plans return a planning error.

## Identity and provenance

The plan retains resolved manifest, engine, workload, input, output, resource,
timeout, concurrency, and repetition settings. Each job identifies its engine,
workload, input, proof mode, repetition, and position within that definition.
Referenced adapter manifests, workload specifications, input fixtures, and
expected outputs have SHA-256 digests, computed once per distinct resolved path.
Fixture contents are never embedded in the plan.

The plan ID is SHA-256 of compact JSON for the tuple
`("zkperf-plan-v1", effective manifest, ordered file provenance)`.
File provenance is ordered by resolved path. This binds CLI/environment overrides
and current referenced file contents, rather than rereading the source TOML and
mistaking its unmodified values for the effective settings. Job IDs are
`<plan ID>:<position>`. No clock, random source, or adapter output is consulted.
Resolved absolute paths are part of provenance: relocating a suite changes its
plan ID. Identical effective definitions and referenced bytes produce identical
plans at the same location. File changes after planning require future execution
to verify digests; planning does not freeze the filesystem.

## CLI and verification

`zkperf run --dry-run` prints the complete plan as pretty JSON to stdout by
default. `--plan-format json|table` selects JSON or an aligned schedule table.
The table shows the plan ID, manifest path, seed, counts, and every job's
position, kind, repetition, engine, workload, input, and proof mode. A job's ID
can be recovered as `<plan ID>:<position>`; JSON carries complete provenance.
Both views use the existing configuration precedence and redaction rules. Dry-run conflicts
with `--print-config`, never discovers or launches adapters, and creates no
output files or directories. Report `--format` selections remain configuration
for future results and do not select the dry-run view. `--plan-format` requires
`--dry-run`. `validate` has no `--dry-run` flag.
Actual execution remains unavailable until its execution services exist.

Tests cover fixed ordering vectors, matrix cardinality, unique/stable IDs,
warm-up exclusion markers, zero warm-ups, proof-mode expansion, provenance and
content changes, effective overrides, count overflow, redaction, CLI conflicts,
equivalent JSON/table schedules, and successful dry-runs with unusable adapter
manifests and executable adapter canaries that must never be invoked.
