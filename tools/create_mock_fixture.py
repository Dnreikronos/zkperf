"""Create a runnable mock benchmark with an explicit local Python command."""

import argparse
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def create_fixture(destination):
    destination = Path(destination).absolute()
    if destination.exists() and (not destination.is_dir() or any(destination.iterdir())):
        raise ValueError("Destination must be absent or an empty directory.")
    destination.mkdir(parents=True, exist_ok=True)
    for name in ("zkperf.toml", "workload.md", "input.txt", "output.txt"):
        shutil.copyfile(ROOT / "examples" / "mock" / name, destination / name)
    manifest = {"kind": "zkperf-adapter-manifest", "manifest_version": "1.0.0",
                "adapter_id": "mock", "display_name": "zkperf deterministic mock adapter",
                "protocol_versions": ["1.0.0"],
                "command": [str(Path(sys.executable).absolute()), "-B",
                            str(ROOT / "adapters" / "mock" / "adapter.py")]}
    (destination / "mock.zkperf-adapter.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return destination / "zkperf.toml"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    try:
        print(create_fixture(arguments.directory))
    except (ValueError, OSError) as error:
        parser.exit(1, "mock fixture: " + str(error) + "\n")


if __name__ == "__main__":
    main()
