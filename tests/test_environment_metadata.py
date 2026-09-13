import copy
import json
import unittest
from pathlib import Path

from tools.validate_benchmark_reports import validate_report

ROOT = Path(__file__).resolve().parents[1]


class EnvironmentMetadataTests(unittest.TestCase):
    def test_report_metadata_gaps_require_reasons(self):
        schema = json.loads((ROOT / "schemas/benchmark-report-v1.schema.json").read_text())
        gap = {"availability": "unavailable", "reason": {
            "code": "unsupported_platform", "message": "Host API does not expose this field."}}
        for example in ["successful", "failed", "timed-out", "partially-supported"]:
            report = json.loads((ROOT / f"examples/reports/{example}.json").read_text())
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
                host["ram_bytes"].pop("reason")
                self.assertTrue(validate_report(report, schema))
