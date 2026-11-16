"""The only writer of job lifecycle state, shared by daemon and supervisor.

The lock covers spawn and registration. A reservation is never permission to
execute: only the registered process can atomically consume the execution token.
Unknown starts are fenced, not replayed, including records from older workers.
"""

import os
import time
from pathlib import Path
from filelock import FileLock
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.core.lifecycle import TERMINAL
from wukong_worker.platform.process_control import process_identity


class JobLedger:
    PRIVATE = {"runner_token", "_ledger"}
    INTERNAL_FILES = {
        "spec.json",
        "status.json",
        "heartbeat.json",
        "cancel",
        "supervisor.log",
        "ledger.lock",
        "supervisor-start.json",
    }

    def __init__(self, directory: Path):
        self.directory = directory
        self.path = directory / "status.json"
        self.lock = FileLock(directory / "ledger.lock", timeout=15)

    def _write(self, value):
        ledger = value.setdefault("_ledger", {})
        ledger["version"] = ledger.get("version", 0) + 1
        atomic_json(self.path, value)

    @classmethod
    def public(cls, value):
        return {key: item for key, item in value.items() if key not in cls.PRIVATE}

    def reserve_and_spawn(self, spec, meta, spawn):
        with self.lock:
            if self.path.exists():
                raise ValueError("job already reserved")
            atomic_json(self.directory / "spec.json", spec)
            meta["_ledger"] = {"schema": 1, "phase": "reserved", "version": 0}
            self._write(meta)
            try:
                process = spawn()
                pid, identity = self._registration(process.pid, spec["runner_token"])
                # Re-read even while holding the lock: no stale snapshot is a
                # transition authority, and failed publication must fail closed.
                fresh = load_json(self.path)
                if fresh != meta or fresh["state"] in TERMINAL:
                    raise RuntimeError("job reservation changed before registration")
                fresh.update(supervisor_pid=pid, supervisor_identity=identity)
                fresh["_ledger"]["phase"] = "registered"
                self._write(fresh)
                return self.public(fresh)
            except Exception:
                fresh = load_json(self.path)
                self._lose(fresh, "supervisor_registration_unconfirmed")
                raise

    def announce(self, token):
        # Windows venv Python can use a launcher parent; register the interpreter
        # that will actually execute, bound to the spawned process and token.
        atomic_json(
            self.directory / "supervisor-start.json",
            {
                "runner_token": token,
                "pid": os.getpid(),
                "parent_pid": os.getppid(),
                "identity": process_identity(os.getpid()),
            },
        )

    def _registration(self, spawned_pid, token):
        deadline = time.monotonic() + 10
        path = self.directory / "supervisor-start.json"
        while time.monotonic() < deadline:
            if path.exists():
                hello = load_json(path)
                identity = process_identity(hello.get("pid"))
                if (
                    hello.get("runner_token") == token
                    and spawned_pid in {hello.get("pid"), hello.get("parent_pid")}
                    and identity not in {"unknown", "missing"}
                    and hello.get("identity") == identity
                ):
                    return hello["pid"], identity
                raise RuntimeError("supervisor creation identity mismatch")
            time.sleep(0.01)
        raise RuntimeError("supervisor registration timed out")

    def claim(self, token, pid):
        with self.lock:
            meta = load_json(self.path)
            ledger = meta.get("_ledger", {})
            identity = process_identity(pid)
            if (
                ledger.get("schema") != 1
                or not isinstance(ledger.get("version"), int)
                or ledger.get("phase") != "registered"
                or meta["state"] != "queued"
                or meta.get("runner_token") != token
                or meta.get("supervisor_pid") != pid
                or identity in {"unknown", "missing"}
                or meta.get("supervisor_identity") != identity
            ):
                return None
            if (self.directory / "cancel").exists():
                meta.update(state="cancelled", finished_at=time.time())
                ledger["phase"] = "finished"
                meta.pop("runner_token", None)
                self._write(meta)
                return None
            ledger["phase"] = "claimed"
            meta.update(state="running", started_at=time.time())
            self._write(meta)
            return ledger["version"]

    def finish(self, token, version, state, **result):
        if state not in TERMINAL:
            raise ValueError("supervisor must publish a terminal outcome")
        with self.lock:
            meta = load_json(self.path)
            ledger = meta.get("_ledger", {})
            if (
                meta["state"] != "running"
                or ledger.get("phase") != "claimed"
                or ledger.get("version") != version
                or meta.get("runner_token") != token
            ):
                return False
            meta.update(result, state=state, finished_at=time.time())
            ledger["phase"] = "finished"
            meta.pop("runner_token", None)
            self._write(meta)
            return True

    def _lose(self, meta, reason):
        if meta["state"] not in TERMINAL:
            meta.update(
                state="lost",
                finished_at=time.time(),
                cleanup_pending=True,
                cleanup_confirmed=False,
                reconciliation=reason,
                error="supervisor outcome unconfirmed; execution is fenced and cleanup is required",
            )
            meta.setdefault("_ledger", {})["phase"] = "revoked"
            meta.pop("runner_token", None)
            self._write(meta)
        return meta

    def observe(self):
        with self.lock:
            meta = load_json(self.path)
            if meta["state"] in TERMINAL:
                return self.public(meta)
            if meta.get("_ledger", {}).get("phase") not in {"registered", "claimed"}:
                return self.public(self._lose(meta, "unregistered_start_requires_recovery"))
            current = process_identity(meta.get("supervisor_pid"))
            expected = meta.get("supervisor_identity")
            if current == "missing" or (current != "unknown" and current != expected):
                return self.public(
                    self._lose(meta, "supervisor_creation_identity_absent_or_changed")
                )
            if current == "unknown":
                meta["reconciliation"] = "supervisor_liveness_unconfirmed"
            return self.public(meta)

    def cancel(self):
        with self.lock:
            if load_json(self.path)["state"] not in TERMINAL:
                (self.directory / "cancel").touch()

    def import_snapshot(self):
        with self.lock:
            meta = load_json(self.path)
            if meta["state"] not in TERMINAL or meta["state"] == "lost":
                meta.update(
                    state="lost",
                    cleanup_pending=False,
                    cleanup_confirmed=True,
                    reconciliation="imported_snapshot_requires_new_execution",
                )
            for key in self.PRIVATE | {"supervisor_pid", "supervisor_identity"}:
                meta.pop(key, None)
            meta["_ledger"] = {"schema": 1, "phase": "imported", "version": 0}
            self._write(meta)
