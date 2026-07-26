# Development baseline

## Toolchain and MSRV

The workspace uses Rust 2024 and declares Rust 1.85 as its minimum supported
Rust version (MSRV). CI checks that MSRV explicitly. Day-to-day development and
the main quality gate use Rust 1.97.1, pinned in
[`rust-toolchain.toml`](../rust-toolchain.toml), with rustfmt and Clippy.

Raising the MSRV is a compatibility change. Update `rust-version`, this
document, and the MSRV CI job together.

Python 3.10 or newer is required for the contract conformance tooling.

## Supported host platforms

The following host configurations are tested in CI:

| Operating system | Architecture | GitHub runner |
| --- | --- | --- |
| Ubuntu 24.04 | x86_64 | `ubuntu-24.04` |
| macOS 15 | arm64 | `macos-15` |
| Windows Server 2025 | x86_64 | `windows-2025` |

Other Rust-supported hosts may work, but are not part of the compatibility
guarantee until they are added to the platform matrix.

## Workspace boundaries

Production dependencies flow in one direction:

```text
zkperf-cli  ->  zkperf-core  ->  zkperf-adapter-protocol
```

- `zkperf-cli` owns user interaction and delegates work to the core.
- `zkperf-core` owns engine-independent orchestration.
- `zkperf-adapter-protocol` owns only the shared subprocess contract.
- Engine adapters run out of process. They may depend on their zkVM SDK and the
  adapter protocol, but never on the CLI or core.

No zkVM SDK may enter the CLI or core dependency graph. The verification script
enforces the exact initial workspace topology, so dependency changes require an
intentional policy update instead of silently crossing a boundary.

## One-command verification

Install the development prerequisites once:

```console
python -m pip install -r requirements-dev.txt
cargo install cargo-audit --version 0.22.2 --locked
```

Then run the same complete gate used by CI:

```console
python tools/check.py
```

The command checks workspace dependency direction, rustfmt, Clippy with
warnings denied, Rust tests, contract conformance tests, checked-in examples,
and the Cargo.lock vulnerability audit.
