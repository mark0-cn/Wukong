"""Detached job ownership, heartbeat and final publication; execution is injected."""

import os
import threading
import time
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.resources.job_ledger import JobLedger


def supervise_job(root, directory, execute):
    spec = load_json(directory / "spec.json")
    token = spec["runner_token"]
    ledger = JobLedger(directory)
    ledger.announce(token)
    version = ledger.claim(token, os.getpid())
    if version is None:
        return
    stop = threading.Event()

    def heartbeat():
        while not stop.wait(0.5):
            atomic_json(
                directory / "heartbeat.json",
                {"runner_token": token, "pid": os.getpid(), "at": time.time()},
            )

    atomic_json(
        directory / "heartbeat.json", {"runner_token": token, "pid": os.getpid(), "at": time.time()}
    )
    threading.Thread(target=heartbeat, daemon=True).start()
    try:
        outcome = execute(root, directory, spec)
        state, result = outcome.state, {"result": outcome.result}
    except Exception as error:
        # The supervisor owns final job publication, including unexpected executor
        # failures; a protocol client must not be required to keep this owner alive.
        state, result = "failed", {"error": str(error)[:4096]}
    finally:
        stop.set()
    ledger.finish(token, version, state, **result)
