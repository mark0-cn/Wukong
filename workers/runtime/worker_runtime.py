"""Worker composition and method routing."""
import os
import sys
from worker_capabilities import doctor
from worker_commands import CommandService
from worker_common import PROTOCOL, TERMINAL
from worker_emu import EmuService
from worker_files import FileService
from worker_jobs import JobManager
from worker_libc import LibcService
from worker_seccomp import SeccompService
from worker_profile import ProfileService
from worker_crash import CrashService
from worker_operations import OperationLedger
from worker_sessions import SessionManager

class Runtime:
    def __init__(self, root, trusted):
        self.root, self.trusted = root.resolve(), trusted
        self.isolated = sys.platform.startswith("linux") and os.geteuid() == 0 and not trusted
        self.jobs = JobManager(self.root, trusted, self.isolated)
        self.sessions = SessionManager(self.root, trusted, self.isolated)
        self.files = FileService(self.root, trusted, self.isolated)
        self.commands = CommandService(self.root, trusted, self.isolated)
        self.emu = EmuService(self.root, trusted, self.isolated)
        self.libc = LibcService(self.root, trusted, self.isolated)
        self.profile = ProfileService(self.root, trusted, self.isolated)
        self.crash = CrashService(self.root, trusted, self.isolated)
        self.seccomp = SeccompService(self.root, trusted, self.isolated)
        self.operations = OperationLedger(self.root)
        self.handlers = {}
        for service in [self.jobs, self.sessions, self.files, self.commands, self.emu, self.libc, self.profile, self.crash, self.seccomp]:
            for name in service.methods:
                if name in self.handlers:
                    raise ValueError("duplicate worker method: " + name)
                self.handlers[name] = service.dispatch

    def dispatch(self, method, params):
        return self.operations.invoke(method, params, self.dispatch_inner, self.sessions.reconcile_reply)

    def dispatch_inner(self, method, params):
        if method in self.handlers:
            return self.handlers[method](method, params)
        if method == "ping":
            return {"protocol": PROTOCOL, "daemon_pid": os.getpid(), "state_dir": str(self.root)}
        if method == "doctor":
            return doctor(self.root,self.trusted,self.isolated)
        if method == "ghidra.query":
            from ghidra_backend import query
            return query(self.root, params)
        if method == "worker.shutdown":
            active = [self.jobs.status(path.parent.name) for path in (self.root / "jobs").glob("*/status.json")]
            if any(job["state"] not in TERMINAL for job in active):
                raise ValueError("cannot shut down worker while jobs are active; cancel and reconcile them first")
            self.sessions.close_all()
            return {"state": "shutdown"}
        raise ValueError("unsupported worker method: " + method)
