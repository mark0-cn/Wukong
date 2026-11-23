"""Shared test client; never part of the runtime service."""

import base64
import json
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time


class Client:
    """Serial test RPCs; owns the stdio relay, never the persistent daemon."""

    def __init__(self, state, environment=None, trusted_local=False):
        command = [
            sys.executable,
            str(Path(__file__).resolve().parents[2] / "runtime" / "bridge.py"),
            "--state",
            str(state),
            "--stdio",
        ]
        if trusted_local:
            command.append("--trusted-local")
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=environment
        )
        self.counter = 0
        self.closed = False
        self.exchange = None
        self.last_telemetry = None

    def call(self, method, *, rpc_timeout=None, traceparent=None, **params):
        if self.closed:
            raise AssertionError("test relay is closed; reconnect explicitly")
        self.counter += 1
        request_id = self.counter
        replies = queue.Queue(maxsize=1)

        def exchange():
            try:
                envelope = {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": params,
                }
                if traceparent is not None:
                    envelope["_wukong_telemetry"] = {"traceparent": traceparent}
                self.process.stdin.write(json.dumps(envelope) + "\n")
                self.process.stdin.flush()
                raw = self.process.stdout.readline()
                if not raw:
                    raise AssertionError(f"test relay ended before {method} reply {request_id}")
                replies.put(json.loads(raw))
            except Exception as error:
                replies.put(error)

        # A stalled write is just as capable of hanging a test as a stalled read.
        # Timeout terminates this relay, leaving daemon/job reconciliation to the test.
        self.exchange = threading.Thread(target=exchange, daemon=True)
        self.exchange.start()
        timeout = rpc_timeout
        if timeout is None:
            timeout = max(
                60,
                (params.get("timeout_secs") or 0) + 30,
                (params.get("read_timeout_secs") or 0) + 30,
            )
        try:
            reply = replies.get(timeout=max(0, timeout))
        except queue.Empty as error:
            self.close()
            raise AssertionError(
                f"test RPC {method} reply {request_id} exceeded {timeout}s"
            ) from error
        self.exchange.join(timeout=5)
        if isinstance(reply, Exception):
            self.close()
            raise reply
        if reply.get("id") != request_id:
            self.close()
            raise AssertionError(
                f"test RPC {method} expected reply {request_id}, got {reply.get('id')}"
            )
        assert "error" not in reply, reply
        self.last_telemetry = reply.get("_wukong_telemetry")
        return reply["result"]

    def put(self, path, data, executable=False):
        return self.call(
            "file.put",
            path=path,
            data_base64=base64.b64encode(data).decode(),
            executable=executable,
        )

    def finish(self, job, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = self.call(
                "job.status", job_id=job["job_id"], rpc_timeout=deadline - time.monotonic()
            )
            if status["state"] in {"completed", "failed", "cancelled", "timed_out", "lost"}:
                return status
            time.sleep(min(0.05, max(0, deadline - time.monotonic())))
        raise AssertionError("job remained active past test deadline: " + str(job))

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            if self.process.poll() is None:
                self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        finally:
            if self.exchange is not None:
                self.exchange.join(timeout=5)
            self.process.stdin.close()
            self.process.stdout.close()
