"""Run the complete local quality gate used by CI."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

EXPECTED_DEPENDENCIES = {
    "zkperf-adapter-protocol": set(),
    "zkperf-core": {"zkperf-adapter-protocol"},
    "zkperf-cli": {"zkperf-core"},
}


def find_command(name: str) -> str:
    """Find a Rust tool in PATH or rustup's default user-local bin directory."""
    executable = f"{name}.exe" if sys.platform == "win32" else name
    found = shutil.which(executable)
    if found is not None:
        return found

    rustup_bin = Path.home() / ".cargo" / "bin" / executable
    if rustup_bin.is_file():
        return str(rustup_bin)

    raise SystemExit(f"required command not found: {name}")


def run(label: str, command: list[str]) -> None:
    """Run one visible check and stop at the first failure."""
    print(f"\n==> {label}", flush=True)
    try:
        subprocess.run(command, cwd=ROOT, check=True)
    except FileNotFoundError as error:
        raise SystemExit(f"required command not found: {command[0]}") from error


def capture(command: list[str]) -> str:
    """Run a command whose standard output is machine-readable."""
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError as error:
        raise SystemExit(f"required command not found: {command[0]}") from error
    return result.stdout


def verify_dependency_boundaries(cargo: str) -> None:
    """Keep engine SDKs outside the CLI and core dependency graphs."""
    print("\n==> Workspace dependency boundaries", flush=True)
    metadata = json.loads(
        capture([cargo, "metadata", "--format-version", "1", "--locked"])
    )
    workspace_ids = set(metadata["workspace_members"])
    packages = {
        package["name"]: package
        for package in metadata["packages"]
        if package["id"] in workspace_ids
    }

    if set(packages) != set(EXPECTED_DEPENDENCIES):
        raise SystemExit(
            "workspace members changed; update the documented dependency policy "
            "and this check deliberately"
        )

    workspace_package_names = set(packages)
    for package_name, expected in EXPECTED_DEPENDENCIES.items():
        dependencies = {
            dependency["name"]
            for dependency in packages[package_name]["dependencies"]
            if dependency["name"] in workspace_package_names
        }
        if dependencies != expected:
            raise SystemExit(
                f"{package_name} dependencies must be {sorted(expected)}, "
                f"found {sorted(dependencies)}"
            )


def main() -> int:
    """Run every required project check."""
    cargo = find_command("cargo")
    try:
        cargo_audit = find_command("cargo-audit")
    except SystemExit as error:
        raise SystemExit(
            "cargo-audit is required; install it with "
            "`cargo install cargo-audit --version 0.22.2 --locked`"
        ) from error

    audit_version = capture([cargo_audit, "--version"]).strip()
    if audit_version != "cargo-audit 0.22.2":
        raise SystemExit(
            f"cargo-audit 0.22.2 is required, found {audit_version!r}"
        )

    verify_dependency_boundaries(cargo)
    run("Rust formatting", [cargo, "fmt", "--all", "--", "--check"])
    run(
        "Clippy",
        [
            cargo,
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )
    run(
        "Rust tests",
        [cargo, "test", "--workspace", "--all-features", "--locked"],
    )
    run(
        "Contract conformance tests",
        [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
    )
    run(
        "BenchmarkReport examples",
        [
            sys.executable,
            "tools/validate_benchmark_reports.py",
            *map(str, sorted((ROOT / "examples/reports").glob("*.json"))),
        ],
    )
    run(
        "Adapter protocol examples",
        [
            sys.executable,
            "tools/validate_adapter_protocol.py",
            "examples/protocol-v1.json",
        ],
    )
    run(
        "Dependency audit",
        [cargo_audit, "audit", "--deny", "warnings"],
    )
    print("\nAll checks passed.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
