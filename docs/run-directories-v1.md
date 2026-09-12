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

Creation opens the canonical suite path one component at a time without
following links. Each output directory and the new run directory is created
relative to its open parent. The run retains its directory handle throughout
execution; its absolute path is used for diagnostics and adapter requests.

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
  opens it once without following links, requests nonblocking I/O, and rejects
  non-regular handles before hashing. A FIFO without a writer is rejected
  instead of blocking adoption. The digest describes the bytes read through
  that handle; callers must finish writing evidence before adopting it.

`run.json`, `plan.json`, and `artifacts.jsonl` are reserved at the run root,
including ASCII case variants such as `ARTIFACTS.JSONL`. This prevents adoption
of mutable internal files through aliases on case-insensitive filesystems.

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

Lexical validation happens before filesystem changes. Parent directories are
opened component by component with no-follow semantics, using `cap-std` and
`cap-fs-ext`. Each operation holds its resolved parent open through creation,
publication, cleanup, and syncing. Replacing that parent's name with a symlink
cannot redirect the operation; on Unix it continues in the original directory
even if that directory has been renamed. Windows directory handles prevent
such renames while open. Adoption also refuses a link in the final component.

Before each operation the root's current device/inode identity is compared
with the retained handle, detecting replacement by another real directory as
well as a link. This is a diagnostic check; handle-relative operations provide
protection against swaps after the check. These guarantees govern harness I/O,
not arbitrary filesystem mutations made by another process with write access.

## Atomicity and interrupted runs

Canonical results are written to a temporary file in the destination directory
and flushed before they are published under their own name; on Unix the
directory is flushed afterwards. A reader never sees a partial file.

Artifacts are published by linking the completed temporary file to its
destination, which fails when that name is already taken. A rename would report
success after replacing whatever another writer published first, and checking
for the destination beforehand only narrows that window instead of closing it.
Only the run's own record is rewritten by rename, because replacing it is the
point. A failed write removes the temporary file it created, and only that one:
a name another writer holds is stepped over rather than deleted.

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
stored artifact or a destination that appeared first, interrupted evidence
readable through `RunRecord`, path escapes through relative segments, absolute
paths, separators, device names and symbolic links, a run root and an output
directory swapped for links after they were checked, refusal of the run's own
files as artifacts, a taken temporary name left alone, digests and byte lengths
for stored and adopted artifacts, disjoint and non-reusable attempt workspaces,
published outcomes and provenance, absence of leftover temporary files, and
redaction of persisted configuration.

Regression tests interleave a parent-directory swap between resolution and
publication, verify retained root identity, reject reserved-name case aliases,
and bound FIFO adoption in a subprocess so regressions cannot hang the suite.

Execution, resource metrics, and report rendering are tracked in issues #11–16.
The CLI writes no run directory until the supervised runner exists.
