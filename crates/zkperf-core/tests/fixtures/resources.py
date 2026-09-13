"""Controlled resource consumers for the supervised collector tests."""

import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time


def consume():
    """Keep touched memory and completed I/O observable while burning CPU."""
    memory = bytearray(64 * 1024 * 1024)
    with open("resource-data", "wb", buffering=0) as output:
        output.write(memory[:8 * 1024 * 1024])
        os.fsync(output.fileno())
    until = time.process_time() + .3
    while time.process_time() < until:
        sum(range(1000))
    time.sleep(1.5)
    assert len(memory) == 64 * 1024 * 1024


mode = sys.argv[1]
if mode == "consumer":
    # Linux must include I/O from non-leader threads without reaped-child totals.
    thread = threading.Thread(target=consume)
    thread.start()
    thread.join()
    sys.exit(0)
if mode == "middle":
    subprocess.run([sys.executable, __file__, "consumer"], check=True)
    time.sleep(.3)
    sys.exit(0)

request = json.load(sys.stdin)
if mode == "grace":
    memory = bytearray(8 * 1024 * 1024)
    Path(os.environ["PID_FILE"]).write_text("ready")
    while not Path("control/cancel.json").exists():
        time.sleep(.002)
    memory = bytearray(128 * 1024 * 1024)
    time.sleep(.3)
else:
    subprocess.run([sys.executable, __file__, "middle"], check=True)
    # This forces another opportunity to sample Linux's parent after wait().
    time.sleep(.5)
    if mode == "failure":
        sys.exit(7)

catalog = json.loads(Path(sys.argv[-1]).read_text())
response = next(item["response"] for item in catalog["exchanges"]
                if item["request"]["operation"] == request["operation"])
response.update({key: request[key] for key in
                 ("protocol", "protocol_version", "request_id", "operation")})
print(json.dumps(response))
