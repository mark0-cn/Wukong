"""Sessions."""
import base64
import os
import shutil
import threading
from worker_common import WorkerError, atomic_json, bounded_int, load_json
from worker_debugger import DebugSession
from worker_interactive import InteractSession

class SessionManager:
    methods = frozenset(['interact.start', 'interact.close', 'interact.write', 'debug.start', 'debug.close', 'debug.command', 'debug.events'])

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.lock = threading.RLock()
        self.debug, self.interact = {}, {}
        # GDB cannot be resurrected after its owner dies. Jobs are independent
        # supervisors and are reconciled by heartbeat identity instead.
        for path in (root / "debug").glob("*/session.json") if (root / "debug").exists() else []:
            try:
                state = load_json(path)
                if state["state"] == "running":
                    state["state"] = "lost"
                    atomic_json(path, state)
            except (ValueError, OSError):
                pass
        for path in (root / "interact").glob("*/session.json") if (root / "interact").exists() else []:
            try:
                state = load_json(path)
                if state["state"] == "running":
                    state["state"] = "lost"
                    atomic_json(path, state)
            except (ValueError, OSError):
                pass

    def reconcile_reply(self, method, result):
        registry = self.debug if method == "debug.start" else self.interact if method == "interact.start" else None
        if registry is not None:
            with self.lock:
                if result["session_id"] not in registry:
                    return {**result, "state":"lost", "reason":"session owner daemon restarted"}
        return result

    def close_all(self):
        for registry in [self.debug, self.interact]:
            for session in registry.values():
                session.close()

    def dispatch(self, method, params):
        if method == "interact.start":
            if not self.trusted and not self.isolated:
                raise ValueError("production interactive sessions require a root Linux worker with isolated processes")
            session = InteractSession(self.root, params, self.isolated)
            self.interact[session.id] = session
            return {"session_id": session.id, "state": "running",
                    "ownership": "single_agent_serialized", "pid": session.process.pid}
        if method in {"interact.write", "interact.close"}:
            session = self.interact.get(params.get("session_id"))
            if session is None:
                raise WorkerError("session_lost", "interactive session absent or lost")
            if method == "interact.close":
                self.interact.pop(session.id, None)
                return session.close()
            data = base64.b64decode(params.get("data_base64", ""), validate=True)
            if len(data) > 1024 * 1024:
                raise ValueError("interactive write exceeds 1 MiB")
            return session.write(data, bounded_int(params.get("read_timeout_secs"), 10, 1, 300))
        if method == "debug.start":
            if not self.trusted and not self.isolated:
                raise ValueError("production debugger requires a root Linux worker with isolated inferiors")
            if not shutil.which("gdb"):
                raise ValueError("GDB is unavailable in this worker")
            session = DebugSession(self.root, params, self.isolated)
            with self.lock:
                self.debug[session.id] = session
            return {"session_id": session.id, "state": "running", "ownership": "single_worker_serialized", "daemon_pid": os.getpid()}
        if method in {"debug.command", "debug.close", "debug.events"}:
            with self.lock:
                session = self.debug.get(params["session_id"])
            if not session:
                raise WorkerError("session_lost", "debug session not owned by this daemon; it is absent or lost")
            if method == "debug.close":
                return session.close()
            if method == "debug.events":
                return {"session_id": session.id, "events": list(session.events), "state": "lost" if session.dead else "running"}
            return session.command(params["command"], bounded_int(params.get("timeout_secs"), 30, 1, 60))
        raise ValueError("unsupported SessionManager method: " + method)
