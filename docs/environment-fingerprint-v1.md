# Environment fingerprint and execution configuration

Issue #13 captures evidence for later report generation (#16). Capture must
survive failed adapter startup, failed preparation, timeout, and cancellation.

## Capture contract

- `run.json` includes an environment fingerprint from run creation onward.
  Host collection is best effort and never substitutes zero, a guessed core
  count, or an empty accelerator list for an unavailable measurement.
- Known metadata serializes as its existing JSON value. An unavailable field
  serializes as `{ "availability": "unavailable", "reason": { "code": "...",
  "message": "..." } }`. Existing complete report examples remain valid.
- Host evidence includes OS/version/kernel, architecture, CPU model/stepping
  and physical/logical cores, installed RAM, and accelerator inventory where
  supported. Hostnames, machine serials, user names, and hardware UUIDs are not
  collected. Unsupported platform fields carry reasons.
- The harness records its package version, compile-time Rust toolchain, target,
  build profile, lockfile digest, and executable digest. No timestamp or run ID
  participates in a fingerprint. Unavailable source revisions remain explicit.
- Requested resources and job selection remain in the existing redacted plan;
  they are declarations, not evidence that limits were enforced. Capture records
  only explicitly supplied adapter environment settings: the runner clears its
  inherited environment. Numeric thread settings and boolean tuning settings
  have a fixed name and value allowlist. Arbitrary flags, paths, credentials,
  and inherited variables are excluded.
- Each attempt records adapter/engine/SDK/toolchain/backend/proof-system/verifier
  identity after successful negotiation and guest artifact digests after build.
  Each snapshot is immutable and indexed with a content digest. Metadata queried
  after preparation receives prepared artifacts. Unreached stages have explicit
  unavailable metadata; earlier snapshots survive a subsequent failure.
- The report schema and Rust metadata types accept the same unavailable form.
  Missing disclosure is evidence of a compatibility gap, not proof of compliance.

## Sources and platform gaps

`sysinfo` 0.33 supplies OS, kernel, CPU brands and
logical CPU counts, and installed RAM on Linux, macOS, and Windows. Physical
core counts use host APIs on macOS/Windows and require complete socket/core IDs
in Linux `/proc/cpuinfo`; logical processors are never substituted for missing
physical topology. No process, user,
network, or hostname inventory is requested. On Linux, `/proc/cpuinfo` supplies
stepping/microcode and `/sys/bus/pci/devices` supplies display-controller and
processing-accelerator PCI class/vendor/device IDs in sorted order. This PCI
inventory does not claim coverage of non-PCI accelerators or remote devices.
An inaccessible or malformed PCI entry makes the inventory unavailable. On
Unix, `rustix::system::uname` supplies the OS-reported architecture. A build
target is never substituted when host architecture cannot be detected.

Stepping, microcode, and accelerator discovery on macOS/Windows are explicitly
unsupported in this collector. Host architecture on Windows is also explicitly
unavailable; the harness build target remains recorded separately. Storage
inventory is not collected. Machine
IDs and arbitrary compiler flags are deliberately excluded. `std::time::Instant`
provides monotonic timing but does not expose clock resolution; nanosecond
serialization does not imply one-nanosecond resolution.

The environment allowlist is `RAYON_NUM_THREADS`, `OMP_NUM_THREADS`,
`OPENBLAS_NUM_THREADS`, `MKL_NUM_THREADS`, and `NUMEXPR_NUM_THREADS` (positive
decimal `u32` values), plus `OMP_DYNAMIC` and `OMP_NESTED` (`true`, `false`,
`TRUE`, or `FALSE`). Invalid values receive an `excluded` reason. Other names
and their values are omitted. The manifest independently rejects secret-like
environment names. The existing protocol requires adapters to return non-secret
metadata; runtime configuration snapshots also redact secret-like keys recursively.

## Persisted layout and hashes

`run.json.environment` is the complete `EnvironmentCapture`.
`run.json.environment_digest` hashes its compact JSON serialization. Field
order is defined by the Rust structs, map keys are sorted, and no timestamps,
run identifiers, free-memory readings, or CPU-frequency readings enter it.
The harness executable digest identifies the executable file observed at
capture time; the lockfile digest identifies the lockfile embedded at build
time. Source revision is explicitly unavailable when not embedded.

Older run records still load. Missing environment fields deserialize as
`not_recorded` gaps; loading a report never probes the reader's host or invents
metadata for the original run.

`metadata/<ten-digit-job-position>/<stage>.json` contains immutable snapshots
for `initial`, `capabilities`, `negotiated`, each successful preparation stage
(`environment`, `build`, `setup`), and `prepared`. Each snapshot names the job
ID and carries `data` and its SHA-256 `digest`; job ID and stage do not enter
that digest. These paths are also indexed in `artifacts.jsonl`. Preparation
snapshots retain the last validated runtime identity and newly observed guest
artifact hashes. The `prepared` snapshot queries metadata again with all
prepared artifacts. Consumers use the latest successful stage for each job;
the `initial` snapshot remains available even for jobs never reached.

Guest source revision and compiler metadata may be supplied by the adapter's
`metadata.result.extensions["org.zkperf.guest"]` object under `source_revision`
and `compiler`. Compiler metadata requires non-empty `name` and `version`;
missing or malformed fields remain explicitly unavailable. Compiler `flags`
are excluded with a reason. The guest artifact's digest, byte length, and media type come
from the runner-validated `guest_program` artifact, independent of this optional
extension. Workload revision is not substituted for guest source revision.

The captured host is the machine running the harness. It does not describe
remote proving services. For local execution, report generation can reuse
the captured host and clock directly, join the
requested resource profile from the plan, and use these per-job snapshots for
runtime identity. It must preserve gaps and evaluate disclosure separately;
this issue does not generate aggregate reports or enforce resource limits.

## Verification

Focused tests cover real host collection, deterministic hashes, allowlisted
environment values, absent and malformed platform data, report round trips,
success, adapter-startup failure, later failure, timeout, and interrupted runs.
Run the Rust compiler, formatter, Clippy, and affected Rust/Python tests. Keep
full report generation, resource measurement, and comparison evaluation in
their own issues.
