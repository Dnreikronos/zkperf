# Immutable run directories and artifact provenance v1

Issue #10 gives every run its own directory under the manifest's resolved
output directory. A run publishes evidence there while it works; it never
writes outside that directory and never replaces an earlier run's evidence.
`RunDirectory::create` takes a built [plan](benchmark-planning-v1.md) and owns
the resulting directory until it is finished or the process stops.

## Layout

```text
<outputs.directory>/
  <UTC stamp>-<run ID>/
    run.json                       canonical record, published atomically
    plan.json                      the redacted schedule this run expands
    artifacts.jsonl                one JSON artifact record per line
    attempts/<position>-<attempt>/ one adapter invocation
      inputs/                      host-written request inputs
      outputs/                     adapter-written artifacts
      control/                     host-owned cancellation and control files
```

`logs/`, `artifacts/`, and `reports/` are conventional destinations rather than
reserved names: any run-relative path is created on demand. The three attempt
roots are pairwise disjoint, matching the ownership rules in the
[adapter subprocess protocol](adapter-subprocess-protocol-v1.md).

The directory name is the UTC creation stamp `YYYYMMDDTHHMMSSZ` followed by the
run ID. RFC 3339 punctuation is not portable across filesystems, so the stamp
carries no separators. The directory is created exclusively: an existing name
is an error, never a replacement. Two runs started in the same second still get
distinct directories because the run ID differs.

## Identity

Run, attempt, and artifact identifiers are RFC 9562 version 8 UUIDs derived
from SHA-256 over length-prefixed ingredients, so identity follows the run's own
provenance instead of a random source:

| Identifier | Domain | Ingredients |
| --- | --- | --- |
| Run | `zkperf-run-id-v1` | plan ID, creation timestamp, process ID, process-local sequence |
| Attempt | `zkperf-attempt-id-v1` | run ID, job ID, attempt index |
| Artifact | `zkperf-artifact-id-v1` | run ID, run-relative artifact path |

A run ID is therefore reproducible from what the record already states, and two
artifacts in one run share an ID only if they share a path, which the directory
rejects.

## Provenance and integrity

`run.json` records the plan ID, the manifest path, the SHA-256 digest of the
manifest file, and the digest of every file the plan referenced: input
fixtures, expected outputs, workload specifications, and adapter manifests. The
plan ID binds the complete effective configuration, including CLI and
environment overrides. Guest binaries and engine outputs do not exist at
creation time; they are hashed when the run stores or adopts them.

Every artifact is content-addressed as it is recorded, with its SHA-256 digest,
byte length, media type, kind, run-relative URI, and originating attempt. Those
records are exactly the `artifacts` entries of a
[`BenchmarkReport`](benchmark-report-v1.md), so the integrity hashes in a
report are the ones computed when the evidence was written.

Two ways in:

- **Store**: the harness writes bytes to a run-relative path. The file is
  published atomically and hashed from the bytes written.
- **Adopt**: an adapter already wrote a file inside its outputs root. The run
  hashes it in place and records it without copying or rewriting it.

Streamed evidence, such as captured adapter output, is created up front and
adopted once complete. `plan.json` uses the manifest's diagnostic redaction, so
configured secrets and environment values never reach persisted evidence.

## Paths

An artifact path is relative to the run directory and uses `/` separators.
Every segment must be non-empty, at most 255 bytes, and limited to ASCII
letters, digits, `.`, `_`, and `-`. Relative segments, empty segments,
backslashes, trailing dots, and Windows device names such as `nul` or `com1`
are rejected on every platform, so one layout stays portable and needs no URI
or shell escaping.

Resolution refuses a symbolic link at any component, including one planted
inside the run directory, and refuses to adopt anything that is not a regular
file. A rejected path never creates a file or a directory. Path handling is
lexical plus a per-component link check rather than canonicalization, so a
directory is never resolved through a link that a concurrent writer could
replace.

## Atomicity and interrupted runs

Canonical results are written to a temporary file in the destination directory,
flushed, and renamed into place; on Unix the destination directory is flushed
afterwards. A reader observes either the previous file or the complete new one.
A failed write removes its temporary file and leaves the published path
unchanged. Storing an artifact at an occupied path is an error, so atomic
publication never becomes silent replacement.

`run.json` is written with state `in_progress` before any work and rewritten
once, atomically, when the run finishes as `completed` or `failed`. A directory
left in `in_progress` belongs to a run that is still going or that was
interrupted; either way its plan, logs, attempt workspaces, and
`artifacts.jsonl` are intact and readable. A finished record declares the
artifact count, which must match the number of lines in `artifacts.jsonl`;
before that, the index is the authority. An interrupted creation can leave a
directory without `run.json`, which identifies it as a run that never started.

## Verification scope

Tests cover unique directories across runs of one plan, refusal to replace a
stored artifact, interrupted evidence readable through `RunRecord`, path
escapes through relative segments, absolute paths, separators, device names and
symbolic links, digests and byte lengths for stored and adopted artifacts,
disjoint and non-reusable attempt workspaces, published outcomes and
provenance, absence of leftover temporary files, and redaction of persisted
configuration.

Execution, resource metrics, and report rendering are tracked in issues #11–16.
The CLI writes no run directory until the supervised runner exists.
