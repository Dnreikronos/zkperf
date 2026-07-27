import copy
import json
import math
import subprocess
import sys
import tempfile
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

    def test_exact_means_reject_tolerance_sized_errors(self) -> None:
        for observation, incorrect_mean in (
            (1_000_000_000, 1_000_000_001),
            (9_007_199_254_740_992, 9_007_199_254_740_993),
        ):
            report = json.loads((REPORTS / "successful.json").read_text())
            for sample in report["measurements"][1]["samples"][1:]:
                sample["value"] = observation
            statistics = report["measurements"][1]["statistics"]
            for field in ("minimum", "maximum", "median"):
                statistics[field] = observation
            statistics["mean"] = incorrect_mean
            statistics["standard_deviation"] = 0
            statistics["percentiles"] = {
                "p50": observation,
                "p95": observation,
            }

            with self.subTest(observation=observation):
                self.assertTrue(
                    any(
                        "/statistics/mean: summary mismatch" in issue
                        for issue in validate_report(report, self.schema)
                    )
                )

    def test_non_terminating_mean_remains_valid(self) -> None:
        report = json.loads((REPORTS / "successful.json").read_text())
        for sample, value in zip(
            report["measurements"][1]["samples"][1:],
            (0, 0, 1),
            strict=True,
        ):
            sample["value"] = value
        statistics = report["measurements"][1]["statistics"]
        statistics.update(
            {
                "minimum": 0,
                "maximum": 1,
                "median": 0,
                "mean": 1 / 3,
                "standard_deviation": math.sqrt(1 / 3),
                "percentiles": {"p50": 0, "p95": 0.9},
            }
        )

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

    def test_report_level_failure_before_first_attempt(self) -> None:
        report = json.loads((REPORTS / "partially-supported.json").read_text())
        report["measurements"] = [
            measurement
            for measurement in report["measurements"]
            if measurement["availability"] == "unavailable"
        ]
        report["status"] = {
            "outcome": "failed",
            "reason": {
                "code": "harness_start_failed",
                "message": "The harness failed before the first attempt.",
            },
        }

        self.assertEqual([], validate_report(report, self.schema))

    def test_cli_reports_bad_inputs_and_continues(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            missing = temp / "missing.json"
            malformed = temp / "malformed.json"
            malformed.write_text("{", encoding="utf-8")
            invalid_utf8 = temp / "invalid-utf8.json"
            invalid_utf8.write_bytes(b"\xff")

            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools" / "validate_benchmark_reports.py"),
                    str(missing),
                    str(malformed),
                    str(invalid_utf8),
                    str(REPORTS / "successful.json"),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(1, result.returncode)
        self.assertIn(f"{missing}: INVALID", result.stdout)
        self.assertIn(f"{malformed}: INVALID", result.stdout)
        self.assertIn(f"{invalid_utf8}: INVALID", result.stdout)
        self.assertIn("successful.json: valid", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_cli_rejects_non_finite_decoded_mean_without_traceback(self) -> None:
        report = json.loads((REPORTS / "successful.json").read_text())
        report["measurements"][0]["statistics"]["mean"] = "NON_FINITE_MEAN"
        encoded = json.dumps(report).replace('"NON_FINITE_MEAN"', "1e400")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "non-finite-mean.json"
            path.write_text(encoded, encoding="utf-8")
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools" / "validate_benchmark_reports.py"),
                    str(path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(1, result.returncode)
        self.assertIn(f"{path}: INVALID", result.stdout)
        self.assertIn("/statistics/mean: summary mismatch", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_cli_preserves_exact_decimal_means_above_f64_integer_range(self) -> None:
        observation = 9_007_199_254_740_992
        report = json.loads((REPORTS / "successful.json").read_text())
        for sample in report["measurements"][1]["samples"][1:]:
            sample["value"] = observation
        statistics = report["measurements"][1]["statistics"]
        for field in ("minimum", "maximum", "median"):
            statistics[field] = observation
        statistics["mean"] = "EXACT_DECIMAL_MEAN"
        statistics["standard_deviation"] = 0
        statistics["percentiles"] = {
            "p50": observation,
            "p95": observation,
        }
        encoded = json.dumps(report)

        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            valid = temp / "valid-decimal-mean.json"
            valid.write_text(
                encoded.replace('"EXACT_DECIMAL_MEAN"', "9007199254740992.0"),
                encoding="utf-8",
            )
            invalid = temp / "invalid-decimal-mean.json"
            invalid.write_text(
                encoded.replace('"EXACT_DECIMAL_MEAN"', "9007199254740993.0"),
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools" / "validate_benchmark_reports.py"),
                    str(valid),
                    str(invalid),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(1, result.returncode)
        self.assertIn(f"{valid}: valid", result.stdout)
        self.assertIn(f"{invalid}: INVALID", result.stdout)
        self.assertIn("/statistics/mean: summary mismatch", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

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
