import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from jsonschema import Draft202012Validator

from tools.validate_benchmark_reports import validate_report

ROOT = Path(__file__).resolve().parents[1]


class EnvironmentMetadataTests(unittest.TestCase):
    def test_report_metadata_gaps_require_reasons(self):
        schema = json.loads((ROOT / "schemas/benchmark-report-v2.schema.json").read_text())
        Draft202012Validator.check_schema(schema)
        gap = {"availability": "unavailable", "reason": {
            "code": "unsupported_platform", "message": "Host API does not expose this field."}}
        for example in ["successful", "failed", "timed-out", "partially-supported"]:
            report = json.loads((ROOT / f"examples/reports/{example}.json").read_text())
            report["schema_version"] = "2.0.0"
            host = report["environment"]["host"]
            for field in ["machine_id", "architecture", "ram_bytes", "accelerators", "storage", "firmware_or_microcode"]:
                host[field] = copy.deepcopy(gap)
            for field in ["model", "stepping", "physical_cores", "logical_cores"]:
                host["cpu"][field] = copy.deepcopy(gap)
            for field in ["name", "version", "kernel"]:
                host["operating_system"][field] = copy.deepcopy(gap)
            report["environment"]["clock"]["resolution_ns"] = copy.deepcopy(gap)
            with self.subTest(example=example):
                self.assertEqual([], validate_report(report, schema))
                self.assertEqual([], validate_report(report))
                report["schema_version"] = "1.0.0"
                self.assertTrue(validate_report(report))
                report["schema_version"] = "2.0.0"
                host["ram_bytes"].pop("reason")
                self.assertTrue(validate_report(report, schema))

    def test_cli_selects_exact_versions_and_keeps_schema_override(self):
        report = json.loads((ROOT / "examples/reports/successful.json").read_text())
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for version in ["1.0.0", "2.0.0", "3.0.0"]:
                report["schema_version"] = version
                path = Path(directory) / f"{version}.json"
                path.write_text(json.dumps(report))
                paths.append(str(path))
            command = [sys.executable, str(ROOT / "tools/validate_benchmark_reports.py")]
            result = subprocess.run(command + paths, capture_output=True, text=True)
            self.assertEqual(1, result.returncode)
            self.assertIn("1.0.0.json: valid", result.stdout)
            self.assertIn("2.0.0.json: valid", result.stdout)
            self.assertIn("3.0.0.json: INVALID", result.stdout)
            self.assertNotIn("Traceback", result.stderr)
            result = subprocess.run(command + paths[:2] + [
                "--schema", str(ROOT / "schemas/benchmark-report-v1.schema.json")
            ], capture_output=True, text=True)
            self.assertEqual(1, result.returncode)
            self.assertIn("1.0.0.json: valid", result.stdout)
            self.assertIn("2.0.0.json: INVALID", result.stdout)
