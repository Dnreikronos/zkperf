"""Explicit, operation-scoped faults for testing subprocess supervision."""

import json
import os
import signal
import sys
import time

from protocol import PHASES, Failure, encode, operation_key, safe_path

FAULTS = ("none", "error", "unsupported", "exit", "signal", "malformed", "invalid-utf8",
          "trailing", "mismatch", "schema", "empty", "stdout-overflow", "stderr-flood",
          "missing-artifact", "bad-digest", "bad-length", "artifact-escape",
          "timeout", "ignore-cancel", "blocked-input")


def settings(request, arguments):
    options = {"fault": "none", "target": "all", "delay_ms": 0, "memory_bytes": 0}
    configured = request["params"].get("configuration", {}).get("mock", {})
    if not isinstance(configured, dict) or set(configured) - options.keys():
        raise Failure("invalid_configuration", "Unknown mock configuration setting.")
    options.update(configured)
    options.update({key: value for key, value in vars(arguments).items() if value is not None})
    targets = {"all", *PHASES, *(key.split(".")[0] for key in PHASES)}
    if (options["fault"] not in FAULTS or not isinstance(options["target"], str)
            or options["target"] not in targets):
        raise Failure("invalid_configuration", "Unknown mock fault or target.")
    if configured.get("fault") == "blocked-input":
        raise Failure("invalid_configuration", "blocked-input is a command-line-only transport fault.")
    for key, limit in (("delay_ms", 3_600_000), ("memory_bytes", 256 * 1024 * 1024)):
        if type(options[key]) is not int or not 0 <= options[key] <= limit:
            raise Failure("invalid_configuration", "Mock resource setting is out of range.")
    return options


def for_operation(request, options):
    if options["target"] not in ("all", request["operation"], operation_key(request)):
        return {"fault": "none", "delay_ms": 0, "memory_bytes": 0}
    return options


def wait(request, seconds, ignore_cancel=False):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        path = safe_path(request["workspace"]["control_dir"] + "/cancel.json")
        if not ignore_cancel and path.is_file():
            try:
                cancellation = json.loads(path.read_bytes())
            except (ValueError, OSError):
                cancellation = {}
            if (isinstance(cancellation, dict)
                    and all(cancellation.get(key) == request[key] for key in
                    ("protocol", "protocol_version", "request_id"))
                    and cancellation.get("reason") in ("user_cancelled", "deadline_exceeded")):
                raise Failure("cancelled", "Operation cancelled by the host.")
        time.sleep(min(0.005, max(0, deadline - time.monotonic())))


def before(request, options):
    # Touch every page and retain the allocation through response publication.
    memory = bytearray(b"M") * options["memory_bytes"]
    fault = options["fault"]
    if fault in ("timeout", "ignore-cancel", "blocked-input"):
        while True:
            wait(request, 60, ignore_cancel=fault != "timeout")
    wait(request, options["delay_ms"] / 1000)
    if fault in ("error", "unsupported"):
        raise Failure("mock_failure", "Configured mock failure.", fault)
    if fault == "exit":
        sys.exit(7)
    if fault == "signal":
        os.kill(os.getpid(), signal.SIGTERM)
    if fault == "stderr-flood":
        sys.stderr.buffer.write(b"L" * (2 * 1024 * 1024))
        sys.stderr.flush()
    return memory


def publish(response, fault):
    if fault in ("missing-artifact", "bad-digest", "bad-length", "artifact-escape"):
        if not response["artifacts"]:
            raise Failure("invalid_configuration", "This artifact fault requires an artifact-producing operation.")
        entry = response["artifacts"][0]
        if fault == "missing-artifact":
            safe_path(entry["path"]).unlink()
        elif fault == "bad-digest":
            path = safe_path(entry["path"])
            data = path.read_bytes()
            path.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        elif fault == "bad-length":
            with safe_path(entry["path"]).open("ab") as output:
                output.write(b"X")
        else:
            entry["path"] = "outputs/../escaped"
    if fault == "mismatch":
        response["request_id"] = "00000000-0000-4000-8000-000000000000"
    if fault == "schema":
        response.pop("status")
    special = {"empty": b"", "malformed": b"mock diagnostic on stdout\n", "invalid-utf8": b"\xff"}
    if fault == "stdout-overflow":
        sys.stdout.buffer.write(b"X" * (2 * 1024 * 1024))
    else:
        sys.stdout.buffer.write(special.get(fault, encode(response)))
    if fault == "trailing":
        sys.stdout.buffer.write(b"{}\n")
    sys.stdout.flush()
