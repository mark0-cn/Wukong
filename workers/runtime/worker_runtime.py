"""Worker composition and method routing."""

import os
import sys
from pathlib import Path
from typing import Any
from worker_capabilities import doctor
from worker_catalog import register
from worker_commands import CommandService
from worker_contract import PROTOCOL
from worker_common import TERMINAL
from worker_emu import EmuService
from worker_files import FileService
from worker_jobs import JobManager
from worker_libc import LibcService
from worker_seccomp import SeccompService
from worker_network import NetworkService
from worker_profile import ProfileService
from worker_crash import CrashService
from worker_operations import OperationLedger
from worker_sessions import SessionManager
from worker_transfers import TransferService
from worker_identity import WorkerIdentity


class Runtime:
    def __init__(self, root: Path, trusted: bool) -> None:
        self.root, self.trusted = root.resolve(), trusted
        self.stopping = False
        self.identity = WorkerIdentity(self.root)
        self.isolated = sys.platform.startswith("linux") and os.geteuid() == 0 and not trusted
        self.jobs = JobManager(self.root, trusted, self.isolated)
        self.sessions = SessionManager(self.root, trusted, self.isolated)
        self.files = FileService(self.root, trusted, self.isolated)
        self.commands = CommandService(self.root, trusted, self.isolated)
        self.emu = EmuService(self.root)
        self.libc = LibcService(self.root)
        self.profile = ProfileService(self.root, trusted, self.isolated)
        self.crash = CrashService(self.root, trusted, self.isolated)
        self.seccomp = SeccompService(self.root)
        self.network = NetworkService(self.root, trusted, self.isolated)
        self.operations = OperationLedger(self.root)
        self.transfers = TransferService(self.root)
        self.registrations = register(self)
        self.handlers = {
            name: registration.handler for name, registration in self.registrations.items()
        }
        # Capability observation needs the complete registry; inject it only
        # after every service and callable handler has been assembled.
        self.jobs.observe = self._observe_capabilities

    def _observe_capabilities(self) -> dict[str, Any]:
        return doctor(self.root, self.trusted, self.isolated, self.registrations)

    def dispatch(self, method: str, params: dict[str, Any]) -> Any:
        if method not in {"ping", "doctor"} and "_binding" in params:
            self.identity.validate(params["_binding"])
            params = {key: value for key, value in params.items() if key != "_binding"}
        return self.operations.invoke(
            method, params, self.dispatch_inner, self.sessions.reconcile_reply
        )

    def dispatch_inner(self, method: str, params: dict[str, Any]) -> Any:
        return self.handlers[method](params)

    def ping(self, params):
        return {
            "protocol": PROTOCOL,
            "identity": self.identity.value,
            "daemon_id": self.identity.daemon_id,
            "daemon_pid": os.getpid(),
            "state_dir": str(self.root),
        }

    def doctor(self, params):
        return {
            **self._observe_capabilities(),
            "identity": self.identity.value,
            "daemon_id": self.identity.daemon_id,
            "exclusive_run_lease": False,
        }

    def ghidra_query(self, params):
        from ghidra_backend import query

        return query(self.root, params)

    def shutdown(self, params):
        active = [
            self.jobs.status(path.parent.name)
            for path in (self.root / "jobs").glob("*/status.json")
        ]
        if any(job["state"] not in TERMINAL for job in active):
            raise ValueError(
                "cannot shut down worker while jobs are active; cancel and reconcile them first"
            )
        self.sessions.close_all()
        self.stopping = True
        return {"state": "shutdown"}
