"""Fault controls and request-boundary behavior without SDKs or network."""

import copy
import json
import subprocess
import sys
import tempfile
import time
import unittest

from mock_support import ADAPTER, EXAMPLES, SCHEMA, Client

from tools.validate_adapter_protocol import validate_exchange


class MockFaultTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.client = Client(self.temporary.name)

    def start(self, request, arguments):
        directory = self.client.workspace(request)
        process = subprocess.Popen([sys.executable, "-B", str(ADAPTER), *arguments],
                                   cwd=directory, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env={})
        return process, directory

    def test_error_and_unsupported_are_correlated_for_every_stage(self):
        for operation, stage in (("capabilities", None), ("metadata", None),
                                 ("prepare", "environment"), ("prepare", "build"),
                                 ("prepare", "setup"), ("execute", None),
                                 ("prove", "initial"), ("prove", "transform"), ("verify", None)):
            request = copy.deepcopy(next(exchange["request"] for exchange in EXAMPLES["exchanges"]
                                         if exchange["request"]["operation"] == operation
                                         and (stage is None or operation == "prepare"
                                              or exchange["request"]["params"]["stage"] == stage)))
            if stage:
                request["params"]["stage"] = stage
                if stage == "environment":
                    request["params"].pop("cache", None)
            for fault in ("error", "unsupported"):
                with self.subTest(operation=operation, stage=stage, fault=fault):
                    response, _ = self.client.invoke(request, arguments=["--fault", fault])
                    self.assertEqual(fault, response["status"])
                    self.assertEqual([], validate_exchange(request, response, SCHEMA))

    def test_wire_faults_are_literal_and_nonzero_exit_is_distinct(self):
        request = self.client.request("capabilities")
        for fault in ("malformed", "invalid-utf8", "trailing", "empty", "stdout-overflow", "exit"):
            with self.subTest(fault=fault):
                process, _ = self.start(request, ["--fault", fault])
                stdout, _ = process.communicate(json.dumps(request).encode(), timeout=5)
                self.assertEqual(7 if fault == "exit" else 0, process.returncode)
                if fault == "stdout-overflow":
                    self.assertGreater(len(stdout), 1048576)
                elif fault == "exit":
                    self.assertEqual(b"", stdout)
                else:
                    with self.assertRaises((ValueError, UnicodeDecodeError)):
                        json.loads(stdout)

    def test_configuration_selects_only_the_requested_stage(self):
        request = self.client.request("metadata")
        request["params"]["configuration"]["mock"] = {"fault": "error", "target": "prove.initial"}
        response, _ = self.client.invoke(request)
        self.assertEqual("success", response["status"])
        request["params"]["configuration"]["mock"]["target"] = "metadata"
        response, _ = self.client.invoke(request)
        self.assertEqual("error", response["status"])
        response, _ = self.client.invoke(request, arguments=["--fault", "none"])
        self.assertEqual("success", response["status"])

    def test_delay_and_memory_are_applied_without_changing_results(self):
        request = self.client.request("capabilities")
        response, _ = self.client.invoke(request)
        start = time.monotonic()
        delayed, _ = self.client.invoke(request, arguments=["--delay-ms", "120", "--memory-bytes", "1048576"])
        self.assertGreaterEqual(time.monotonic() - start, .12)
        self.assertEqual(response, delayed)
        script = (
            "import sys,tracemalloc; sys.path.insert(0,sys.argv[1]); import faults; "
            "tracemalloc.start(); "
            "memory=faults.before({},dict(fault='none',delay_ms=0,memory_bytes=1048576)); "
            "assert len(memory)==1048576 and all(byte==77 for byte in memory); "
            "assert tracemalloc.get_traced_memory()[0]>=1048576"
        )
        subprocess.run([sys.executable, "-B", "-c", script, str(ADAPTER.parent)], check=True, timeout=5)

    def test_metadata_reports_resolved_resource_overrides(self):
        request = self.client.request("metadata")
        request["params"]["configuration"]["mock"] = {
            "target": "all", "fault": "none", "delay_ms": 0, "memory_bytes": 0}
        original = copy.deepcopy(request)
        response, _ = self.client.invoke(
            request, arguments=["--delay-ms", "120", "--memory-bytes", "1048576"])
        self.assertEqual({"backend": "local-cpu", "mock": {
            "target": "all", "fault": "none", "delay_ms": 120, "memory_bytes": 1048576,
        }}, response["result"]["configuration"])
        self.assertEqual(original, request)

    def test_metadata_preserves_settings_for_other_stages(self):
        for override_target in (False, True):
            with self.subTest(override_target=override_target):
                request = self.client.request("metadata")
                request["params"]["configuration"]["mock"] = {
                    "target": "metadata" if override_target else "prove.initial",
                    "fault": "error", "delay_ms": 0, "memory_bytes": 0}
                arguments = ["--fault", "unsupported", "--delay-ms", "30000", "--memory-bytes", "1048576"]
                if override_target:
                    arguments.extend(["--target", "prove.initial"])
                response, _ = self.client.invoke(request, arguments=arguments)
                self.assertEqual({"backend": "local-cpu", "mock": {
                    "target": "prove.initial", "fault": "unsupported",
                    "delay_ms": 30000, "memory_bytes": 1048576,
                }}, response["result"]["configuration"])

    def test_metadata_includes_defaults_and_explicit_zero_overrides(self):
        request = self.client.request("metadata")
        response, _ = self.client.invoke(request)
        self.assertEqual({"target": "all", "fault": "none", "delay_ms": 0, "memory_bytes": 0},
                         response["result"]["configuration"]["mock"])
        request["params"]["configuration"]["mock"] = {
            "fault": "error", "delay_ms": 30000, "memory_bytes": 1048576}
        overridden, _ = self.client.invoke(request, arguments=[
            "--fault", "none", "--delay-ms", "0", "--memory-bytes", "0"])
        self.assertEqual(response["result"]["configuration"], overridden["result"]["configuration"])

    def test_delay_cancellation_checks_correlation_and_deadline_reason(self):
        for reason in ("user_cancelled", "deadline_exceeded"):
            request = self.client.request("capabilities")
            process, directory = self.start(request, ["--delay-ms", "30000"])
            try:
                cancellation = {key: request[key] for key in ("protocol", "protocol_version", "request_id")}
                cancellation.update(reason=reason, request_id="00000000-0000-4000-8000-000000000000")
                path = directory / "control/cancel.json"
                path.write_text(json.dumps(cancellation))
                process.stdin.write(json.dumps(request).encode())
                process.stdin.close()
                process.stdin = None
                time.sleep(.15)
                self.assertIsNone(process.poll(), "Unrelated cancellation must be ignored")
                cancellation["request_id"] = request["request_id"]
                path.write_text(json.dumps(cancellation))
                stdout, stderr = process.communicate(timeout=3)
                self.assertEqual(0, process.returncode, stderr)
                response = json.loads(stdout)
                self.assertEqual("cancelled", response["error"]["code"])
                self.assertEqual([], validate_exchange(request, response, SCHEMA))
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate()

    def test_invalid_requests_exit_64_without_partial_stdout(self):
        request = self.client.request("capabilities")
        cases = [b"not json", b"\xff", b"{}", b"null", b"[]", b"\xef\xbb\xbf{}"]
        for version in ("9.0.0", None):
            changed = copy.deepcopy(request)
            changed["protocol_version"] = version
            cases.append(json.dumps(changed).encode())
        for payload in cases:
            process, _ = self.start(request, [])
            stdout, stderr = process.communicate(payload, timeout=5)
            self.assertEqual(64, process.returncode, stderr)
            self.assertEqual(b"", stdout)
            self.assertTrue(stderr)

    def test_resource_configuration_rejects_invalid_values(self):
        request = self.client.request("metadata")
        for option in ({"memory_bytes": -1}, {"delay_ms": "1"}, {"fault": "typo"},
                       {"memory_bytes": 268435457}, {"delay_ms": True}, {"typo": 1},
                       {"target": "prove.typo"}, {"target": []}, {"fault": "blocked-input"}):
            request["params"]["configuration"]["mock"] = option
            response, _ = self.client.invoke(request)
            self.assertEqual("invalid_configuration", response["error"]["code"])

    def test_workspace_roots_reject_escapes_and_overlaps(self):
        for output_root in ("inputs", "inputs/nested", "../outside", "/tmp/outside"):
            request = self.client.request("capabilities")
            process, directory = self.start(request, [])
            (directory / "inputs/nested").mkdir()
            request["workspace"]["outputs_dir"] = output_root
            stdout, stderr = process.communicate(json.dumps(request).encode(), timeout=5)
            self.assertEqual(64, process.returncode, stderr)
            self.assertEqual(b"", stdout)

    @unittest.skipIf(sys.platform == "win32", "Symlink creation may require Windows privileges")
    def test_linked_output_root_is_rejected_before_writing(self):
        request = self.client.request("prepare", stage="build", input_artifacts=[])
        process, directory = self.start(request, [])
        outside = directory / "outside"
        outside.mkdir()
        (directory / "outputs").rmdir()
        (directory / "outputs").symlink_to(outside, target_is_directory=True)
        stdout, stderr = process.communicate(json.dumps(request).encode(), timeout=5)
        self.assertEqual(64, process.returncode, stderr)
        self.assertEqual(b"", stdout)
        self.assertEqual([], list(outside.iterdir()))


if __name__ == "__main__":
    unittest.main()
