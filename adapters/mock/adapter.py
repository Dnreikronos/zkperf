"""Run one mock protocol operation. Invoke with an absolute Python executable."""

import argparse
import json
import sys
import time
import zlib

import faults
import workload
from protocol import (
    Failure,
    capabilities,
    encode,
    envelope,
    metadata,
    read_inputs,
    validate_request,
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fault", choices=faults.FAULTS)
    parser.add_argument("--target")
    parser.add_argument("--delay-ms", type=int)
    parser.add_argument("--memory-bytes", type=int)
    arguments = parser.parse_args()
    if arguments.fault == "blocked-input":
        # Bootstrap-only transport fault: deliberately never read stdin.
        while True:
            time.sleep(60)
    try:
        raw = sys.stdin.buffer.read(1048577)
        if len(raw) > 1048576:
            raise ValueError("Request exceeds the mock's one MiB input limit.")
        request = json.loads(raw.decode("utf-8"))
        validate_request(request)
    except (ValueError, KeyError, TypeError, AttributeError, OSError, Failure) as error:
        print("mock: invalid request: " + str(error), file=sys.stderr)
        return 64
    try:
        options = faults.settings(request, arguments)
        memory = faults.before(request, options)
        inputs = read_inputs(request)
        response = envelope(request)
        operation = request["operation"]
        if operation == "capabilities":
            result = capabilities()
        elif operation == "metadata":
            result = metadata(request["params"]["configuration"])
        else:
            result = workload.run(request, response, inputs)
        if operation in ("prepare", "execute", "prove"):
            result["observations"] = {}
        response["result"] = result
        faults.publish(response, options["fault"])
        del memory
    except Failure as error:
        sys.stdout.buffer.write(encode(envelope(request, failure=error)))
    except (ValueError, KeyError, TypeError, AttributeError, OSError, zlib.error) as error:
        failure = Failure("invalid_operation", "Mock operation rejected: " + str(error))
        sys.stdout.buffer.write(encode(envelope(request, failure=failure)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
