"""Shared test client; never part of the runtime service."""
import base64
import json
from pathlib import Path
import subprocess
import sys
import time


class Client:
    def __init__(self, state, environment=None, trusted_local=False):
        command = [sys.executable, str(Path(__file__).resolve().parents[2] / "runtime" / "bridge.py"), "--state", str(state), "--stdio"]
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


