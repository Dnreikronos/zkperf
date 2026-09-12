# CLI contract

The command surface is implemented with Clap. Command handlers translate input
and delegate manifest validation and invariant checks to `zkperf-core`.

## Commands

- `zkperf init [DIRECTORY] [--force]`: initialize a benchmark directory (default
  `.`) without prompting. See [starter generation](init.md) for its files and
  overwrite policy. Success prints a validation command as the next action.
- `zkperf validate [--manifest FILE] [--print-config]`: validate a manifest and
  its effective configuration. Print deterministic, redacted JSON when requested.
- `zkperf run [--manifest FILE] [--dry-run | --print-config]`: resolve benchmark
  configuration. `--dry-run` prints the deterministic schedule; `--print-config`
  prints only the effective configuration. Otherwise execute planned jobs and
  retain subprocess evidence in an immutable run directory.
- `zkperf report INPUT [--format FORMAT] [--output FILE]`: render a stored report.
  Rendering belongs to issues #16, #20 and #21.
- `zkperf compare BASELINE CANDIDATE [--format FORMAT] [--output FILE]`:
  compare stored reports. Comparison and regression policy belong to issue #22.

Until their respective services exist, `report` and `compare` return an explicit
unavailable diagnostic and exit 5 without creating files. All commands provide
`--help`.

`run` executes jobs sequentially, including warm-ups, negotiates capabilities,
and uses fresh subprocesses for preparation, execution, proving, transformations,
and verification. It stops on the first failed or unsupported operation, prints
the retained evidence path, and preserves cancellation as a separate outcome.
Ctrl-C requests cancellation and process-tree cleanup. See the
[runner contract](subprocess-runner-v1.md) for limits and evidence layout.

This execution slice produces operation evidence rather than aggregate benchmark
reports; `--format` is retained in the plan for future report production. Resource
measurements and resource-limit enforcement are not implemented. Combined and
end-to-end timing profiles are rejected explicitly rather than producing
misleading component times.

`validate` and `run` accept `--warmups N`, `--runs N`, `--output-dir DIRECTORY`,
and `--format FORMAT[,FORMAT...]`. Formats are `terminal`, `json`, `html`, `csv`.
Multiple formats replace the whole manifest list; duplicates are invalid.
`report` accepts one format; `compare` accepts `terminal` or `json`.

`run --dry-run` accepts `--plan-format json|table`, defaulting to JSON. JSON
contains every job, the effective redacted manifest, and referenced file digests.
The table summarizes the same ordered jobs, separates warm-ups from measured
repetitions, and shows the plan ID used as the prefix of every job ID. Positions
and repetition indices start at zero. `--plan-format` requires `--dry-run`;
`--dry-run` conflicts with `--print-config`. Report `--format` selections do not
change the plan view. Neither view executes or discovers adapters, writes files,
or creates output directories. See [planning v1](benchmark-planning-v1.md) for
the ordering, identity, and provenance contract.

```console
zkperf run --manifest suite/zkperf.toml --dry-run --plan-format table
zkperf run --manifest suite/zkperf.toml --dry-run --plan-format json > plan.json
```

## Configuration precedence

The first available value wins: **explicit flag > environment > manifest >
CLI default**. Resolve each field independently; a higher-priority source masks
lower-priority values, including malformed environment values. Invalid selected
environment values produce a configuration error, without echoing their value.

| Flag | Environment | Manifest field | CLI default |
| --- | --- | --- | --- |
| `--manifest` | `ZKPERF_MANIFEST` | — | `zkperf.toml` |
| `--warmups` | `ZKPERF_WARMUPS` | `run.warmups` | none |
| `--runs` | `ZKPERF_RUNS` | `run.runs` | none |
| `--output-dir` | `ZKPERF_OUTPUT_DIR` | `outputs.directory` | none |
| `--format` | `ZKPERF_FORMAT` | `outputs.formats` for validate/run | `terminal` for report/compare |
| `--log-level` | `ZKPERF_LOG` | — | `info` |

The manifest remains subject to the complete v1 schema: overrides do not repair
an invalid or incomplete manifest. No benchmark policy defaults are introduced.
Effective values are applied to a single validated manifest before dispatch;
`--print-config` displays that same definition, with the core's redaction rules.
No override rewrites the source manifest.

Manifest paths and report input/output paths are relative to the current working
directory. Benchmark output directories (flags, environment, and manifest) are
relative to the manifest directory and follow the same path restrictions as
`outputs.directory`. Each run writes to its own
directory underneath it; see [run directories](run-directories-v1.md). Comma-separated environment formats replace the whole list.
Unrelated environment variables are ignored. `init` does not read a manifest.
`report` and `compare` do not load one either.

## Diagnostics, logging and exit status

Global options work before or after a subcommand. `-v` / `--verbose` selects
debug logging; `-q` / `--quiet` suppresses progress logs. They conflict with each
other and with `--log-level`. Levels are `off`, `error`, `warn`, `info`, `debug`,
and `trace`. Logging uses stderr, with `zkperf: LEVEL: MESSAGE`. Errors always
remain visible, including in quiet mode, as `zkperf: error[CATEGORY]: MESSAGE`.
Clap usage diagnostics retain its standard usage and help suggestions.
Stdout contains only command results or requested help/version output.

| Exit | Meaning |
| --- | --- |
| 0 | Success, help or version |
| 2 | CLI usage: unknown/missing/conflicting arguments or invalid flag value |
| 3 | Configuration: invalid environment or manifest (including inaccessible manifest files/fixtures) |
| 4 | I/O: initialization conflict, filesystem failure, or writing CLI output failed |
| 5 | Requested service is not implemented |
| 6 | Adapter, protocol, artifact, or execution failure; run evidence is retained |
| 130 | Harness cancellation after supervised cleanup |

Future comparison services must add distinct statuses for incompatible reports
and detected regressions before they are wired in.

## Verification scope

Test every command's help, required arguments, mutually exclusive logging flags,
invalid counts/formats, and global options on either side of the subcommand.
Test source precedence and shadowed malformed environment values in isolated
subprocess environments. Check effective configuration and redaction, path
resolution from another working directory, unchanged source files, stdout/stderr
separation, and success/usage/configuration/I/O/unavailable exit classes. Run
targeted manifest tests, CLI tests, Clippy, compiler and formatting checks, plus
the dependency boundary gate after adding Clap.
