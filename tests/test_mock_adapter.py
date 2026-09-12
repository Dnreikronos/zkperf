"""Offline conformance and deterministic artifact tests for the real mock adapter."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

from mock_support import EXAMPLES, ROOT, SCHEMA, Client, digest

from tools.create_mock_fixture import create_fixture
from tools.validate_adapter_protocol import validate_catalog


class MockAdapterTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.client = Client(self.temporary.name)

    def test_complete_lifecycle_conforms_and_is_byte_reproducible(self):
        self.client.lifecycle()
        second = Client(Path(self.temporary.name) / "unrelated-location")
        second.lifecycle()
        for client in (self.client, second):
            cancellation = copy.deepcopy(EXAMPLES["cancellation"])
            cancellation["request_id"] = client.exchanges[-1]["request"]["request_id"]
            catalog = {"manifest": EXAMPLES["manifest"], "cancellation": cancellation,
                       "exchanges": client.exchanges}
            self.assertEqual([], validate_catalog(catalog, SCHEMA))
        self.assertEqual(9, len(self.client.exchanges))
        for first, repeated in zip(self.client.exchanges, second.exchanges):
            first_response, second_response = first["response"], repeated["response"]
            self.assertNotEqual(first_response["request_id"], second_response["request_id"])
            self.assertEqual(first_response["result"], second_response["result"])
            self.assertEqual(first_response["artifacts"], second_response["artifacts"])
            for artifact in first_response["artifacts"]:
                self.assertEqual((self.client.directory / first["name"] / artifact["path"]).read_bytes(),
                                 (second.directory / repeated["name"] / artifact["path"]).read_bytes())

    def test_verifies_initial_and_compressed_proofs_with_each_commitment_policy(self):
        for input_commitment, output_commitment in ((False, False), (True, False), (False, True), (True, True)):
            with self.subTest(input=input_commitment, output=output_commitment):
                client = Client(Path(self.temporary.name) / f"{input_commitment}-{output_commitment}",
                                {"input": input_commitment, "output": output_commitment})
                verify, _, raw = client.lifecycle()
                verify["params"]["proof"] = raw[0][0]
                response, _ = client.invoke(verify, raw)
                self.assertEqual("accepted", response["result"]["verdict"])

    def test_verifier_rejects_tampered_proof_and_every_statement_component(self):
        verify, proof, raw = self.client.lifecycle()
        mutations = [
            ("statement", {"workload": "wrong"}),
            ("expected_commitment_digests", {}),
            ("expected_output_digest", digest(b"wrong")),
        ]
        for field, value in mutations:
            request = copy.deepcopy(verify)
            request["params"][field] = value
            response, _ = self.client.invoke(request, proof)
            self.assertEqual("verification_failed", response["error"]["code"])
        for field, value in (("case_id", "different"), ("workload_revision", "different"),
                             ("workload_digest", digest(b"different")), ("security_target_bits", 100),
                             ("implementation_lane", "optimized")):
            request = copy.deepcopy(verify)
            request["params"]["benchmark"][field] = value
            response, _ = self.client.invoke(request, proof)
            self.assertEqual("error", response["status"])
        request = copy.deepcopy(verify)
        entry, data = copy.deepcopy(raw[0])
        value = json.loads(data)
        value["payload"]["input_hex"] = b"different".hex()
        data = json.dumps(value).encode()
        entry.update(byte_length=len(data), digest=digest(data))
        request["params"]["proof"] = entry
        response, _ = self.client.invoke(request, [(entry, data)])
        self.assertEqual("invalid_proof", response["error"]["code"])

    def test_execution_recomputes_output_and_validates_input_bytes(self):
        self.client.lifecycle()
        request = copy.deepcopy(self.client.exchanges[5]["request"])
        request["params"]["prepared_artifacts"] = []
        entry = request["params"]["canonical_input"]
        changed = b"X" * entry["byte_length"]
        response, _ = self.client.invoke(request, [(entry, changed)])
        self.assertEqual("invalid_input", response["error"]["code"])
        entry["digest"] = digest(changed)
        response, _ = self.client.invoke(request, [(entry, changed)])
        self.assertEqual("output_mismatch", response["error"]["code"])

    def test_unknown_proof_mode_and_transformation_are_unsupported(self):
        verify, proof, raw = self.client.lifecycle()
        verify["params"]["proof_mode_id"] = "unknown"
        response, _ = self.client.invoke(verify, proof)
        self.assertEqual("unsupported", response["status"])
        request = self.client.request("prove", stage="transform", transformation_id="unknown",
                                      input_artifacts=[raw[0][0]])
        response, _ = self.client.invoke(request, raw)
        self.assertEqual("unsupported", response["status"])

    def test_fixture_materialization_preserves_bytes_and_refuses_overwrite(self):
        directory = Path(self.temporary.name) / "fixture with spaces"
        manifest = create_fixture(directory)
        self.assertEqual((ROOT / "examples/mock/zkperf.toml").read_bytes(), manifest.read_bytes())
        for name in ("workload.md", "input.txt", "output.txt"):
            self.assertEqual((ROOT / "examples/mock" / name).read_bytes(), (directory / name).read_bytes())
        adapter = json.loads((directory / "mock.zkperf-adapter.json").read_bytes())
        self.assertTrue(Path(adapter["command"][0]).is_absolute())
        self.assertTrue(Path(adapter["command"][-1]).is_file())
        with self.assertRaisesRegex(ValueError, "empty directory"):
            create_fixture(directory)


if __name__ == "__main__":
    unittest.main()
