#!/usr/bin/env python3
"""Live Linux acceptance test. Executes only the embedded crash fixture.

Run inside the isolated worker image:
  python3 /opt/re-agent/runtime/integration_test.py --state /state/acceptance
Add --fuzz to require AFL++ QEMU and prove that it executes and emits stats.
"""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

FIXTURE = r'''
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(int argc, char **argv) {
    if (argc < 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 3;
    unsigned char input[128] = {0};
    size_t n = fread(input, 1, sizeof(input), file);
    fclose(file);
    if (n >= 3 && input[0] == 'B' && input[1] == 'A' && input[2] == 'D') abort();
    puts("fixture-ok");
    return 0;
}
'''


class Client:
    def __init__(self, state, environment=None, trusted_local=False):
        command = [sys.executable, str(Path(__file__).with_name("bridge.py")), "--state", str(state), "--stdio"]
        if trusted_local:
            command.append("--trusted-local")
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=environment)
        self.counter = 0

    def call(self, method, **params):
        self.counter += 1
        self.process.stdin.write(json.dumps({"id": self.counter, "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        reply = json.loads(self.process.stdout.readline())
        assert "error" not in reply, reply
        return reply["result"]

    def put(self, path, data, executable=False):
        return self.call("file.put", path=path, data_base64=base64.b64encode(data).decode(), executable=executable)

    def finish(self, job, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = self.call("job.status", job_id=job["job_id"])
            if status["state"] in {"completed", "failed", "cancelled", "timed_out", "lost"}:
                return status
            time.sleep(0.05)
        raise AssertionError("job remained active past test deadline: " + str(job))

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=5)
        self.process.stdin.close()
        self.process.stdout.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--fuzz", action="store_true")
    args = parser.parse_args()
    state = Path(args.state).resolve()
    client = Client(state)
    doctor = client.call("doctor")
    assert doctor["capabilities"]["debug_gdb_mi"], doctor
    source = client.put("fixture.c", FIXTURE.encode())
    # The acceptance harness compiles its own trusted fixture; analysed-target
    # jobs cannot write the worker's private uploaded-file namespace.
    with tempfile.TemporaryDirectory() as build:
        source_path = Path(build) / "fixture.c"
        target_path = Path(build) / "fixture"
        source_path.write_text(FIXTURE)
        subprocess.run(["gcc", "-g", "-O0", "-o", str(target_path), str(source_path)], check=True)
        client.put("fixture", target_path.read_bytes(), executable=True)
    crash = client.put("crash.bin", b"BAD-padding-to-minimize")
    seed = client.put("seeds/seed", b"BAA")
    started = client.call("debug.start", target="files/fixture", args=[str(state / seed["path"])])
    session_id = started["session_id"]
    breakpoint = client.call("debug.command", session_id=session_id, command="-break-insert main")
    assert breakpoint["class"] == "done", breakpoint
    run = client.call("debug.command", session_id=session_id, command="-exec-run")
    assert run["class"] == "running", run
    for _ in range(100):
        events = client.call("debug.events", session_id=session_id)
        if any(record.get("type") == "notify" and record.get("message") == "stopped" for record in events["events"]):
            break
        time.sleep(0.05)
    else:
        raise AssertionError(events)
    stack = client.call("debug.command", session_id=session_id, command="-stack-list-frames")
    assert stack["class"] == "done" and 'func="main"' in stack["record"], stack
    memory = client.call("debug.command", session_id=session_id, command="-data-read-memory-bytes main 16")
    assert memory["class"] == "done" and "contents=" in memory["record"], memory
    # Relay restart must preserve debugger ownership as well as running jobs.
    client.close()
    client = Client(state)
    assert client.call("debug.command", session_id=session_id, command="-stack-list-frames")["class"] == "done"
    client.call("debug.close", session_id=session_id)
    replay = client.finish(client.call("crash.replay", target="files/fixture", input=crash["path"], timeout_secs=3))
    assert replay["result"]["passed"] and replay["result"]["observed_stage"] == "crash", replay
    verified = client.finish(client.call("poc.verify", target="files/fixture", input=crash["path"], trials=3, timeout_secs=3,
                                         producer_agent_id="fixture-producer", verifier_agent_id="independent-test-verifier", call_id="fixture-verification"))
    assert verified["result"]["passed"] and verified["result"]["successes"] == 3, verified
    minimized = client.finish(client.call("input.minimize", target="files/fixture", input=crash["path"], timeout_secs=3, max_attempts=100))
    assert minimized["result"]["minimized_size"] == 3, minimized
    data = client.call("file.get", path=minimized["result"]["artifact"]["path"])
    assert base64.b64decode(data["data_base64"]) == b"BAD", data
    results = {"doctor": doctor, "gdb": {"breakpoint": breakpoint, "stack": stack, "memory": memory}, "replay": replay,
               "verification": verified, "minimization": minimized}
    if args.fuzz:
        # Docker Desktop can pipe core notifications to an external handler.
        # This fixture check proves QEMU execution/statistics, not complete crash
        # collection under that host setting. Never mutate host-global sysctls.
        fuzz = client.call("fuzz.start", target="files/fixture", seeds="files/seeds", timeout_secs=8, exec_timeout_ms=1000, memory_mb=256,
                           env={"AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES": "1"})
        fuzz = client.finish(fuzz, timeout=40)
        assert fuzz["state"] == "completed" and fuzz["fuzzer_stats"], fuzz
        assert any(int(stats.get("execs_done", "0")) > 0 for stats in fuzz["fuzzer_stats"].values()), fuzz
        results["fuzz"] = fuzz
    output = state / "acceptance-results.json"
    output.write_text(json.dumps(results, indent=2))
    print(json.dumps({"passed": True, "results": str(output), "fuzz_tested": args.fuzz, "verified_trials": 3, "minimized_bytes": 3}))
    client.close()


if __name__ == "__main__":
    main()
