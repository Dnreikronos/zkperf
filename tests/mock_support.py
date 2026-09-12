"""Fresh-process protocol client used only by mock conformance tests."""

import copy
import hashlib
import json
import subprocess
import sys
import uuid
from pathlib import Path

from tools.validate_adapter_protocol import validate_exchange

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / "adapters/mock/adapter.py"
SCHEMA = json.loads((ROOT / "schemas/adapter-protocol-v1.schema.json").read_text())
EXAMPLES = json.loads((ROOT / "examples/protocol-v1.json").read_text())


def digest(data):
    return {"algorithm": "sha256", "value": hashlib.sha256(data).hexdigest()}


class Client:
    def __init__(self, directory, policy=None):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.counter = 0
        self.exchanges = []
        self.data = (ROOT / "examples/mock/input.txt").read_bytes()
        self.benchmark = {
            "case_id": "mock-sha256-small", "workload_revision": "fixture-v1",
            "workload_digest": digest((ROOT / "examples/mock/workload.md").read_bytes()),
            "implementation_lane": "portable", "security_target_bits": 128,
            "expected_output_digest": digest((ROOT / "examples/mock/output.txt").read_bytes()),
            "commitment_policy": policy or {"input": True, "output": True}}

    def request(self, operation, **params):
        request = copy.deepcopy(next(exchange["request"] for exchange in EXAMPLES["exchanges"]
                                     if exchange["request"]["operation"] == operation))
        request["request_id"] = str(uuid.uuid4())
        request["params"].update(params)
        if "benchmark" in request["params"]:
            request["params"]["benchmark"] = copy.deepcopy(self.benchmark)
        if "configuration" in request["params"]:
            request["params"]["configuration"] = {"backend": "local-cpu"}
        return request

    def workspace(self, request, inputs=()):
        self.counter += 1
        directory = self.directory / str(self.counter)
        directory.mkdir()
        for root in request["workspace"].values():
            (directory / root).mkdir(parents=True)
        for entry, data in inputs:
            path = directory / entry["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return directory

    def invoke(self, request, inputs=(), arguments=(), conform=True):
        directory = self.workspace(request, inputs)
        process = subprocess.run([sys.executable, "-B", str(ADAPTER), *arguments], cwd=directory,
                                 input=json.dumps(request).encode(), capture_output=True,
                                 timeout=5, env={}, check=False)
        if process.returncode != 0:
            raise AssertionError((process.returncode, process.stderr))
        response = json.loads(process.stdout)
        if conform:
            issues = validate_exchange(request, response, SCHEMA)
            if issues:
                raise AssertionError(issues)
        artifacts = []
        for entry in response["artifacts"]:
            data = (directory / entry["path"]).read_bytes()
            assert digest(data) == entry["digest"]
            assert len(data) == entry["byte_length"]
            artifacts.append((entry, data))
        self.exchanges.append({"name": str(self.counter), "request": request, "response": response})
        return response, artifacts

    @staticmethod
    def stage(artifacts):
        staged = []
        for index, (entry, data) in enumerate(artifacts):
            entry = copy.deepcopy(entry)
            entry["id"] = "input-" + str(index)
            entry["path"] = "inputs/" + entry["id"]
            staged.append((entry, data))
        return staged

    def lifecycle(self):
        self.invoke(self.request("capabilities"))
        self.invoke(self.request("metadata"))
        prepared = []
        for stage in ("environment", "build", "setup"):
            inputs = self.stage(prepared)
            request = self.request("prepare", stage=stage, input_artifacts=[e for e, _ in inputs])
            if stage == "environment":
                request["params"].pop("cache", None)
            _, artifacts = self.invoke(request, inputs)
            prepared.extend(artifacts)
        canonical = ({"id": "canonical-input", "kind": "canonical_input", "path": "inputs/canonical",
                      "media_type": "application/octet-stream", "byte_length": len(self.data),
                      "digest": digest(self.data)}, self.data)
        inputs = self.stage(prepared)
        execute, artifacts = self.invoke(self.request(
            "execute", canonical_input=canonical[0], prepared_artifacts=[e for e, _ in inputs]),
            [*inputs, canonical])
        trace = next(item for item in artifacts if item[0]["kind"] == "execution_trace")
        inputs = self.stage([*prepared, trace])
        _, artifacts = self.invoke(self.request("prove", stage="initial",
                                               input_artifacts=[e for e, _ in inputs]), inputs)
        raw_proof = next(item for item in artifacts if item[0]["kind"] == "proof")
        inputs = self.stage([*prepared, raw_proof])
        _, artifacts = self.invoke(self.request("prove", stage="transform", transformation_id="mock-compress",
                                               input_artifacts=[e for e, _ in inputs]), inputs)
        proof = self.stage(artifacts)
        commitments = execute["result"]["commitment_digests"]
        verify = self.request("verify", proof=proof[0][0],
                              expected_output_digest=self.benchmark["expected_output_digest"],
                              expected_commitment_digests=commitments,
                              statement={"workload": self.benchmark["case_id"],
                                         "input_commitment": commitments.get("input", {}).get("value"),
                                         "output_commitment": commitments.get("output", {}).get("value")})
        self.invoke(verify, proof)
        return verify, proof, self.stage([raw_proof])
