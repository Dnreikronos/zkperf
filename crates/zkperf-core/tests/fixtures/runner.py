"""Subprocess failure fixtures; this is not a benchmark adapter."""

import json
import hashlib
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

mode = sys.argv[1]
if mode == "descendant":
    Path(sys.argv[2]).write_text(str(os.getpid()))
    time.sleep(2)
    Path(sys.argv[3]).write_text("survived")
    time.sleep(30)
    sys.exit(0)
if mode == "blocked-input":
    time.sleep(30)
    sys.exit(0)

request = json.load(sys.stdin)
sys.stderr.buffer.write(b"adapter diagnostic\n")
sys.stderr.flush()
if mode in {"tree", "orphan-pipes", "orphan-silent", "graceful", "cancel-cli"}:
    handles = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL} if mode == "orphan-silent" else {}
    child = subprocess.Popen([sys.executable, __file__, "descendant",
                              os.environ["PID_FILE"], os.environ["SURVIVOR_FILE"]], **handles)
    while not Path(os.environ["PID_FILE"]).exists():
        time.sleep(.002)
    if mode in {"orphan-pipes", "orphan-silent"}:
        sys.exit(0)
    if mode == "graceful":
        while not Path("control/cancel.json").exists():
            time.sleep(.002)
        child.terminate()
        child.wait()
        time.sleep(.15)
    else:
        time.sleep(30)
if mode == "sleep":
    time.sleep(30)
if mode == "exit":
    sys.exit(7)
if mode == "signal":
    os.kill(os.getpid(), signal.SIGTERM)
if mode == "flood":
    sys.stderr.buffer.write(b"L" * (2 * 1024 * 1024))
if mode == "overflow":
    sys.stdout.buffer.write(b"X" * (2 * 1024 * 1024))
    sys.stdout.flush()
    time.sleep(30)
if mode == "malformed":
    print("SDK progress is not protocol JSON")
    sys.exit(0)
if mode == "invalid-utf8":
    sys.stdout.buffer.write(b"\xff")
    sys.exit(0)
if mode == "environment":
    assert "HOME" not in os.environ
    assert os.environ["EXPLICIT_VALUE"] == "literal $VALUE"
    assert Path.cwd().name == request["request_id"]
    assert sys.argv[2] == "$(echo unexpanded)"

catalog = json.loads(Path(sys.argv[-1]).read_text())
response = next(item["response"] for item in catalog["exchanges"]
                if item["request"]["operation"] == request["operation"])
response.update({key: request[key] for key in
                 ("protocol", "protocol_version", "request_id", "operation")})
if mode in {"lifecycle", "bad-artifact", "oversized-artifact"}:
    params = request["params"]
    for key in ("input_artifacts", "prepared_artifacts", "artifacts", "canonical_input", "proof"):
        entries = params.get(key, [])
        if isinstance(entries, dict):
            entries = [entries]
        for entry in entries:
            data = Path(entry["path"]).read_bytes()
            assert len(data) == entry["byte_length"]
            assert hashlib.sha256(data).hexdigest() == entry["digest"]["value"]
    operation = request["operation"]
    result = response["result"]
    if operation == "capabilities":
        result["proof_modes"][0]["id"] = "default"
    if operation == "prepare":
        result["stage"] = params["stage"]
        result["prepared_artifact_ids"] = ["prepared-" + params["stage"]]
        response["artifacts"][0]["id"] = result["prepared_artifact_ids"][0]
    if operation == "prove":
        result["stage"] = params["stage"]
        result["proof_mode_id"] = params["proof_mode_id"]
        if params["stage"] == "transform":
            result["transformation_id"] = params["transformation_id"]
            result.pop("public_values_artifact_id", None)
            response["artifacts"] = response["artifacts"][:1]
            response["artifacts"][0]["media_type"] = "application/vnd.zkperf.mock-proof+compressed"
    if operation == "verify":
        result["output_digest"] = params["expected_output_digest"]
        result["commitment_digests"] = params["expected_commitment_digests"]
    for artifact in response["artifacts"]:
        if artifact["kind"] == "canonical_output":
            data = (Path(__file__).parent / "manifest/output.bin").read_bytes()
        else:
            data = artifact["id"].encode()
        artifact["path"] = "outputs/" + artifact["id"]
        artifact["byte_length"] = len(data)
        artifact["digest"]["value"] = hashlib.sha256(data).hexdigest()
        Path(artifact["path"]).write_bytes(data)
        if mode == "bad-artifact":
            Path(artifact["path"]).write_bytes(b"X" * len(data))
        if mode == "oversized-artifact":
            Path(artifact["path"]).write_bytes(data * 1000)
    if operation == "execute":
        result["commitment_digests"] = {
            "input": params["canonical_input"]["digest"],
            "output": params["benchmark"]["expected_output_digest"],
        }
if mode == "mismatch":
    response["request_id"] = "10000000-0000-4000-8000-000000000099"
if mode in {"error", "unsupported", "graceful"}:
    response.pop("result", None)
    response["status"] = "unsupported" if mode == "unsupported" else "error"
    response["error"] = {"phase": "capabilities", "code": "cancelled",
                         "message": "fixture failure", "retryable": False}
print(json.dumps(response))
if mode == "trailing":
    print("{}")
