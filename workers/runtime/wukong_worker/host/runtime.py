"""Worker composition and method routing."""

import os
import sys
import threading
from pathlib import Path
from typing import Any
from wukong_worker.host.capabilities import doctor
from wukong_worker.rpc.catalog import register
from wukong_worker.execution.commands import CommandService
from wukong_worker.core.contract import PROTOCOL
from wukong_worker.analysis.adapters import EmuService
from wukong_worker.artifacts.files import FileService
from wukong_worker.resources.jobs import JobManager
from wukong_worker.resources.lifecycle import shutdown_worker
from wukong_worker.operations.jobs import JobService
from wukong_worker.analysis.adapters import LibcService
from wukong_worker.analysis.adapters import SeccompService
from wukong_worker.network.service import NetworkService
from wukong_worker.analysis.profile import ProfileService
from wukong_worker.debugger.crash import CrashService
from wukong_worker.resources.operations import OperationLedger
from wukong_worker.resources.sessions import SessionManager
from wukong_worker.artifacts.transfers import TransferService
from wukong_worker.host.identity import WorkerIdentity


class Runtime:
    def __init__(self, root: Path, trusted: bool) -> None:
        self.root, self.trusted = root.resolve(), trusted
        self.stopping = False
        self._admission = threading.RLock()
        self._active_requests = 0
        self.identity = WorkerIdentity(self.root)
        self.isolated = sys.platform.startswith("linux") and os.geteuid() == 0 and not trusted
        self.jobs = JobManager(self.root, trusted, self.isolated)
        self.job_tools = JobService(self.root, trusted, self.isolated, self.jobs)
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
        self.job_tools.observe = self._observe_capabilities

    def _observe_capabilities(self) -> dict[str, Any]:
        return doctor(self.root, self.trusted, self.isolated, self.registrations)

    def dispatch(self, method: str, params: dict[str, Any]) -> Any:
        with self._admission:
            if self.stopping:
                raise ValueError("worker is shutting down")
            self._active_requests += 1
        try:
            return self._dispatch(method, params)
        finally:
            with self._admission:
                self._active_requests -= 1

    def _dispatch(self, method: str, params: dict[str, Any]) -> Any:
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
        from wukong_worker.analysis.ghidra import query

        return query(self.root, params)

    def shutdown(self, params):
        with self._admission:
            if self._active_requests > 1:
                raise ValueError("cannot shut down worker while requests own active resources")
            shutdown_worker(self.root, self.jobs, self.sessions)
            self.stopping = True
            return {"state": "shutdown"}
