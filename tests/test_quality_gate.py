"""Regression tests for repository policy checks."""

from __future__ import annotations

import unittest

from tools.check import EXPECTED_DIRECT_DEPENDENCIES, verify_package_dependencies


class DependencyBoundaryTests(unittest.TestCase):
    def test_rejects_unlisted_registry_dependency(self) -> None:
        packages = {
            package_name: {
                "dependencies": [
                    {"name": dependency_name}
                    for dependency_name in sorted(expected_dependencies)
                ]
            }
            for package_name, expected_dependencies in EXPECTED_DIRECT_DEPENDENCIES.items()
        }
        packages["zkperf-core"]["dependencies"].append({"name": "risc0-zkvm"})

        with self.assertRaisesRegex(SystemExit, "risc0-zkvm"):
            verify_package_dependencies(packages)


if __name__ == "__main__":
    unittest.main()
