"""Wire helpers for the standalone, standard-library-only mock adapter."""

import hashlib
import json
import os
import platform
import stat
import uuid
import zlib
from pathlib import Path

IDENTITY = {"id": "mock", "name": "zkperf deterministic mock adapter",
            "version": "1.0.0", "revision": "mock-adapter-v1"}
PROOF_FORMAT = "application/vnd.zkperf.mock-proof"
COMPRESSED_FORMAT = PROOF_FORMAT + "+compressed"
MAX_ARTIFACT_BYTES = 16 * 1024 * 1024
PHASES = {"capabilities": "capabilities", "metadata": "metadata",
          "prepare.environment": "preparation", "prepare.build": "build",
          "prepare.setup": "setup", "execute": "execution",
          "prove.initial": "proving", "prove.transform": "compression",
          "verify": "verification"}


class Failure(Exception):
    def __init__(self, code, message, status="error", phase=None):
        super().__init__(message)
        self.code, self.status, self.phase = code, status, phase


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True, allow_nan=False).encode("utf-8") + b"\n"


def digest(data):
    return {"algorithm": "sha256", "value": hashlib.sha256(data).hexdigest()}


def operation_key(request):
    stage = request["params"].get("stage")
    return request["operation"] + ("." + stage if isinstance(stage, str) and stage else "")


def safe_path(value):
    if (not isinstance(value, str) or not value or "\\" in value
            or ":" in value or "\0" in value
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise Failure("invalid_path", "Expected a relative workspace path.", phase="artifact")
    path = Path(value)
    current = Path.cwd()
    for part in path.parts:
        current = current / part
        if current.is_symlink() or current.is_mount():
            raise Failure("invalid_path", "Linked workspace paths are forbidden.", phase="artifact")
    if not path.resolve().is_relative_to(Path.cwd().resolve()):
        raise Failure("invalid_path", "Path escapes the workspace.", phase="artifact")
    return path


def validate_request(request):
    if (request["protocol"] != "zkperf-adapter"
            or request["protocol_version"] != "1.0.0"
            or request["operation"] not in {key.split(".")[0] for key in PHASES}
            or str(uuid.UUID(request["request_id"])) != request["request_id"]
            or not isinstance(request["params"], dict)):
        raise ValueError("Invalid request envelope or unsupported protocol version.")
    roots = [request["workspace"][field] for field in
             ("inputs_dir", "outputs_dir", "control_dir")]
    resolved = [safe_path(root).resolve() for root in roots]
    for root in resolved:
        if not root.is_dir():
            raise ValueError("Workspace roots must be existing directories.")
    if any(a.is_relative_to(b) or b.is_relative_to(a)
           for i, a in enumerate(resolved) for b in resolved[i + 1:]):
        raise ValueError("Workspace roots must be disjoint.")
    for field in ("limit_ns", "termination_grace_ns"):
        value = request["timeout"][field]
        if type(value) is not int or value < (1 if field == "limit_ns" else 0):
            raise ValueError("Invalid timeout.")


def envelope(request, result=None, failure=None):
    response = {key: request[key] for key in
                ("protocol", "protocol_version", "request_id", "operation")}
    response["artifacts"] = []
    if failure is None:
        response.update(status="success", result=result)
    else:
        response.update(status=failure.status, error={
            "phase": failure.phase or PHASES.get(operation_key(request), "protocol"),
            "code": failure.code, "message": str(failure), "retryable": False})
    return response


def capabilities():
    operations = {op: {"supported": True} for op in
                  ("capabilities", "metadata", "prepare", "execute", "prove", "verify")}
    operations["prepare"]["stages"] = ["environment", "build", "setup"]
    operations["prove"]["stages"] = ["initial", "transform"]
    boundaries = []
    for key, phase in PHASES.items():
        if phase in ("capabilities", "metadata", "preparation"):
            continue
        op, _, stage = key.partition(".")
        boundary = {"operation": op, "phase": {"kind": "standard", "name": phase}}
        if stage:
            boundary["stage"] = stage
        boundaries.append(boundary)
    return {"adapter": IDENTITY, "supported_protocol_versions": ["1.0.0"],
            "operations": operations, "measurement_boundaries": boundaries,
            "proof_modes": [{"id": "mock-core", "display_name": "Deterministic mock proof",
                             "proof_format": PROOF_FORMAT, "proof_system": "deterministic-hash-proof",
                             "backend": "local-cpu", "features": ["compression"],
                             "transformations": [{"id": "mock-compress", "kind": "compression",
                                                  "output_format": COMPRESSED_FORMAT}],
                             "verifier": "mock-verifier-v1"}],
            "cancellation": {"graceful": True, "mechanism": "control_file"},
            "limits": {"max_protocol_stdout_bytes": 1048576, "max_artifact_count": 32,
                       "max_artifact_bytes": MAX_ARTIFACT_BYTES,
                       "max_total_artifact_bytes": 2 * MAX_ARTIFACT_BYTES}}


def metadata(configuration):
    def component(name, version):
        return {"status": "available", "name": name, "version": version}
    return {"adapter": IDENTITY,
            "engine": {"id": "mock-engine", "name": "zkperf mock engine", "version": "1.0.0"},
            "sdk": {"status": "unavailable", "reason": {
                "code": "not_applicable", "message": "No SDK is used by the mock adapter."}},
            "toolchain": component("python", platform.python_version()),
            "proof_system": component("deterministic-hash-proof", "1"),
            "backend": component("local-cpu-zlib", zlib.ZLIB_RUNTIME_VERSION),
            "verifier": component("mock-verifier", "1"), "configuration": configuration}


def read_inputs(request):
    params = request["params"]
    entries = []
    for key in ("artifacts", "input_artifacts", "prepared_artifacts"):
        entries.extend(params.get(key, []))
    for key in ("canonical_input", "proof"):
        if key in params:
            entries.append(params[key])
    inputs = {}
    for entry in entries:
        if entry["id"] in inputs:
            raise Failure("invalid_input", "Input artifact IDs must be unique.", phase="artifact")
        if not entry["path"].startswith(request["workspace"]["inputs_dir"] + "/"):
            raise Failure("invalid_path", "Artifact is outside inputs_dir.", phase="artifact")
        path = safe_path(entry["path"])
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_ARTIFACT_BYTES:
            raise Failure("invalid_input", "Expected a bounded regular input file.", phase="artifact")
        data = path.read_bytes()
        if len(data) != entry["byte_length"] or digest(data) != entry["digest"]:
            raise Failure("invalid_input", "Input length or digest mismatch.", phase="artifact")
        inputs[entry["id"]] = (entry, data)
    return inputs


def artifact(request, response, name, kind, data, media_type="application/octet-stream"):
    if len(data) > MAX_ARTIFACT_BYTES:
        raise Failure("artifact_too_large", "Mock artifact exceeds its advertised limit.", phase="artifact")
    relative = request["workspace"]["outputs_dir"] + "/" + name
    path = safe_path(relative)
    temporary = safe_path(relative + ".tmp")
    with temporary.open("xb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)
    response["artifacts"].append({"id": name, "kind": kind, "path": relative,
                                  "media_type": media_type, "byte_length": len(data),
                                  "digest": digest(data)})
    return name
