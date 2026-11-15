"""Detached job ownership, heartbeat and final publication; execution is injected."""

import os
import threading
import time
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.platform.process_control import process_identity


def supervise_job(root, directory, execute):
    spec = load_json(directory / "spec.json")
    meta = load_json(directory / "status.json")
    token = spec["runner_token"]
    meta.update(
        {
            "state": "running",
            "supervisor_pid": os.getpid(),
            "supervisor_identity": process_identity(os.getpid()),
            "started_at": time.time(),
            "runner_token": token,
        }
    )
    atomic_json(directory / "status.json", meta)
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
        meta.update(state=outcome.state, result=outcome.result)
    except Exception as error:
        # The supervisor owns final job publication, including unexpected executor
        # failures; a protocol client must not be required to keep this owner alive.
        meta.update({"state": "failed", "error": str(error)[:4096]})
    finally:
        stop.set()
        meta["finished_at"] = time.time()
        atomic_json(directory / "status.json", meta)
