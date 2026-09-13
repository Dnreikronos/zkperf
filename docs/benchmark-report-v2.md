# BenchmarkReport v2

Model version `2.0.0` uses
[`benchmark-report-v2.schema.json`](../schemas/benchmark-report-v2.schema.json).
It keeps the measurements, outcomes, units, and comparison rules from
[v1](benchmark-report-v1.md), and adds explicit unavailable environment values.
The published `1.0.0` schema and existing v1 examples remain unchanged.

The following fields accept either their original value or an unavailable object:

- Host: `machine_id`, `architecture`, `ram_bytes`, `accelerators`, `storage`,
  and optional `firmware_or_microcode`.
- CPU: `model`, `stepping`, `physical_cores`, and `logical_cores`.
- Operating system: `name`, `version`, and `kernel`.
- Clock: `resolution_ns`.

```json
{
  "availability": "unavailable",
  "reason": {
    "code": "not_exposed",
    "message": "The host API does not expose this field."
  }
}
```

A reason is required. Null, zero counts, guessed hardware, and empty inventories
must not stand in for missing observations. A gap does not satisfy disclosure or
prove comparability; consumers still need to evaluate it. The
[capture contract](environment-fingerprint-v1.md) describes collection sources
and platform limits.

Reports containing these objects must declare `schema_version: "2.0.0"`.
Adding object values to scalar fields is a breaking contract change, even though
complete v1 values are also valid in a report labeled v2. Consumers select the
exact version before parsing and reject versions they don't support. Reading a
report never silently changes its version.

Rust producers use `BenchmarkReportV1::new` for v1 or `BenchmarkReportV2::new` for
v2. Both accept the same parts; the v1 constructor rejects unavailable metadata.
`BenchmarkReport::as_v1()` now returns `Option<&BenchmarkReportV1>`, and `as_v2()`
returns the corresponding v2 reference. Both typed deserializers reject the
other version. The Python validator selects a schema from `schema_version` by
default; `--schema` still allows an explicit schema override.

Environment captures retain their independent `capture_version: "1.0.0"`.
Report generation remains in issue #16; this adds the contract and typed API
needed to preserve captured gaps when that work is implemented.
