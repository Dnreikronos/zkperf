import copy
import json
import unittest
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator

from tools.validate_benchmark_reports import validate_report

ROOT = Path(__file__).resolve().parents[1]

SCHEMA_PATH = ROOT / "schemas" / "benchmark-report-v1.schema.json"
REPORTS = ROOT / "examples" / "reports"
INVALID_CASES = ROOT / "tests" / "fixtures" / "invalid-reports.json"


def _resolve_pointer(document: Any, pointer: str) -> tuple[Any, str]:
    parts = [
        part.replace("~1", "/").replace("~0", "~")
        for part in pointer.removeprefix("/").split("/")
    ]
    parent = document
    for part in parts[:-1]:
        parent = parent[int(part)] if isinstance(parent, list) else parent[part]
    return parent, parts[-1]


def _apply_operation(document: Any, operation: dict[str, Any]) -> None:
    parent, key = _resolve_pointer(document, operation["path"])
    if operation["op"] == "remove":
        if isinstance(parent, list):
            parent.pop(int(key))
        else:
            del parent[key]
        return
    if operation["op"] in {"add", "replace"}:
        if isinstance(parent, list):
            parent[int(key)] = operation["value"]
        else:
            parent[key] = operation["value"]
        return
    raise ValueError(f"unsupported fixture operation: {operation['op']}")


class BenchmarkReportTests(unittest.TestCase):
    schema: dict[str, Any]

    @classmethod
    def setUpClass(cls) -> None:
        cls.schema = json.loads(SCHEMA_PATH.read_text())

    def test_schema_is_valid_draft_2020_12(self) -> None:
        Draft202012Validator.check_schema(self.schema)

    def test_positive_fixtures(self) -> None:
        for path in sorted(REPORTS.glob("*.json")):
            with self.subTest(path=path.name):
                report = json.loads(path.read_text())
                self.assertEqual([], validate_report(report, self.schema))

    def test_phase_specific_success_correctness(self) -> None:
        base = json.loads((REPORTS / "successful.json").read_text())
        for phase_name in ("execution", "build"):
            with self.subTest(phase=phase_name):
                report = copy.deepcopy(base)
                phase = {"kind": "standard", "name": phase_name}
                report["measurements"][0]["phase"] = phase
                for entry in report["run"]["planned_order"]:
                    entry["phase"] = phase
                report["measurements"] = report["measurements"][:1]
                for sample in report["measurements"][0]["samples"]:
                    if phase_name == "execution":
                        sample["correctness"] = {
                            "output_digest": sample["correctness"]["output_digest"]
                        }
                    else:
                        del sample["correctness"]
                self.assertEqual([], validate_report(report, self.schema))

    def test_proof_size_phase_mismatch_is_diagnosable(self) -> None:
        report = json.loads((REPORTS / "successful.json").read_text())
        report["measurements"][1]["phase"] = {
            "kind": "standard",
            "name": "end_to_end",
        }

        issues = validate_report(report, self.schema)

        proof_size_issue = next(
            issue
            for issue in issues
            if "expected one proof_size observation for phase" in issue
        )
        self.assertIn('"name":"proving"', proof_size_issue)
        self.assertIn('"name":"end_to_end"', proof_size_issue)

    def test_negative_fixtures(self) -> None:
        cases = json.loads(INVALID_CASES.read_text())
        for case in cases:
            with self.subTest(case=case["name"]):
                report = json.loads((REPORTS / case["base"]).read_text())
                mutated = copy.deepcopy(report)
                for operation in case["operations"]:
                    _apply_operation(mutated, operation)
                issues = validate_report(mutated, self.schema)
                self.assertTrue(issues, "negative fixture unexpectedly passed")
                self.assertTrue(
                    any(issue.startswith(case["expected_layer"]) for issue in issues),
                    f"expected {case['expected_layer']} failure, got {issues}",
                )


if __name__ == "__main__":
    unittest.main()
