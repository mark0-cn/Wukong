"""Sessions."""

import base64
import logging
import os
from pathlib import Path
import re
import shutil
import sys
import threading
from worker_errors import WorkerError
from worker_metadata import atomic_json, load_json
from wukong_analysis.validation import bounded_int
import worker_debugger
from worker_debugger import DebugSession
from worker_interactive import InteractSession

LOGGER = logging.getLogger(__name__)


def _recover_sessions(directory: Path) -> None:
    """Mark former live owners lost; never repair unreadable records or reopen them."""
    skipped = 0
    for path in directory.glob("*/session.json") if directory.exists() else []:
        try:
            state = load_json(path)
            if state["state"] == "running":
                state["state"] = "lost"
                atomic_json(path, state)
        except (ValueError, OSError) as error:
            # Preserve the existing recovery policy, with bounded diagnostics.
            # Do not log metadata contents, exception messages or arbitrary paths.
            skipped += 1
            if skipped <= 16:
                identifier = path.parent.name
                if not re.fullmatch(r"[0-9a-f]{32}", identifier):
                    identifier = "nonstandard-session-id"
                LOGGER.warning(
                    "Session recovery skipped %s/%s metadata (%s)",
                    directory.name,
                    identifier,
                    type(error).__name__,
                )
    if skipped > 16:
        LOGGER.warning(
            "Session recovery skipped %s additional %s records", skipped - 16, directory.name
        )


class SessionManager:
    """Own live sessions; retain ownership when close fails so cleanup can retry."""

    def __init__(self, root: Path, trusted: bool, isolated: bool) -> None:
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.lock = threading.RLock()
        self.debug: dict[str, DebugSession] = {}
        self.interact: dict[str, InteractSession] = {}
        # GDB cannot be resurrected after its owner dies. Jobs are independent
        # supervisors and are reconciled by heartbeat identity instead.
        _recover_sessions(root / "debug")
        _recover_sessions(root / "interact")

    def reconcile_reply(self, method, result):
        registry = (
            self.debug
            if method == "debug.start"
            else self.interact
            if method == "interact.start"
            else None
        )
        if registry is not None:
            with self.lock:
                if result["session_id"] not in registry:
                    return {**result, "state": "lost", "reason": "session owner daemon restarted"}
        return result

    def close_all(self):
        for registry in [self.debug, self.interact]:
            for session in registry.values():
                session.close()

    def interact_start(self, params):
        if not self.trusted and not self.isolated:
            raise ValueError(
                "production interactive sessions require a root Linux worker with isolated processes"
            )
        session = InteractSession(self.root, params, self.isolated)
        with self.lock:
            self.interact[session.id] = session
        return {
            "session_id": session.id,
            "state": "running",
            "ownership": "single_agent_serialized",
            "pid": session.process.pid,
        }

    def _interact(self, params):
        with self.lock:
            session = self.interact.get(params.get("session_id"))
        if session is None:
            raise WorkerError("session_lost", "interactive session absent or lost")
        return session

    def interact_profile(self, params):
        session = self._interact(params)
        from worker_profile import process_profile

        with session.lock:
            if session.process.poll() is not None:
                raise ValueError("interactive process exited")
            return {"session_id": session.id, "process": process_profile(session.process.pid)}

    def interact_close(self, params):
        session = self._interact(params)
        result = session.close()
        with self.lock:
            self.interact.pop(session.id, None)
        return result

    def interact_write(self, params):
        session = self._interact(params)
        data = base64.b64decode(params.get("data_base64", ""), validate=True)
        if len(data) > 1024 * 1024:
            raise ValueError("interactive write exceeds 1 MiB")
        return session.write(data, bounded_int(params.get("read_timeout_secs"), 10, 1, 300))

    def debug_start(self, params):
        engine = params.get("engine", "auto")
        if engine == "auto":
            engine = "dbgeng" if sys.platform == "win32" else "gdb_mi"
        if engine == "dbgeng":
            raise ValueError(
                "WinDbg uses the external MCP backend; this metadata worker cannot own debugger processes"
            )
        if engine != "gdb_mi":
            raise ValueError("requested debugger engine is unavailable")
        if not self.trusted and not self.isolated:
            raise ValueError(
                "production debugger requires a root Linux worker with isolated inferiors"
            )
        if not shutil.which("gdb"):
            raise ValueError("GDB is unavailable in this worker")
        if worker_debugger.parse_mi_response is None:
            raise ValueError("pygdbmi is not installed in this worker; GDB debug is unavailable")
        session = DebugSession(self.root, params, self.isolated)
        with self.lock:
            self.debug[session.id] = session
        return {
            "session_id": session.id,
            "state": "running",
            "ownership": "single_worker_serialized",
            "daemon_pid": os.getpid(),
        }

    def _debug(self, params):
        with self.lock:
            session = self.debug.get(params["session_id"])
        if not session:
            raise WorkerError(
                "session_lost", "debug session not owned by this daemon; it is absent or lost"
            )

        return session

    def debug_inspect(self, params):
        session = self._debug(params)
        return session.inspect(params, bounded_int(params.get("timeout_secs"), 30, 1, 60))

    def debug_control(self, params):
        session = self._debug(params)
        return session.control(params, bounded_int(params.get("timeout_secs"), 30, 1, 60))

    def debug_snapshot(self, params):
        session = self._debug(params)
        from worker_crash import debugger_snapshot

        return debugger_snapshot(session)

    def debug_profile(self, params):
        session = self._debug(params)
        return session.profile()

    def debug_close(self, params):
        session = self._debug(params)
        return session.close()

    def debug_events(self, params):
        session = self._debug(params)
        return {
            "session_id": session.id,
            "events": list(session.events),
            "state": "lost" if session.dead else "running",
        }

    def debug_command(self, params):
        session = self._debug(params)
        return session.command(
            params["command"], bounded_int(params.get("timeout_secs"), 30, 1, 60)
        )
