"""Sessions."""

import base64
import logging
import os
from pathlib import Path
import re
import shutil
import sys
import threading
from wukong_worker.core.errors import WorkerError
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_analysis.validation import bounded_int
import wukong_worker.debugger.gdb as worker_debugger
from wukong_worker.debugger.gdb import DebugSession
from wukong_worker.debugger.interactive import InteractSession

LOGGER = logging.getLogger(__name__)


def _recover_sessions(directory: Path) -> None:
    """Reconcile durable process ownership before declaring a former owner gone."""
    skipped = 0
    records = set(directory.glob("*/session.json"))
    records.update(path.parent / "session.json" for path in directory.glob("*/processes.json"))
    for path in sorted(records):
        try:
            # A crash between spawn and publishing the session still owns resources.
            state = load_json(path) if path.exists() else {"state": "recovery_required"}
            if state["state"] != "closed":
                from wukong_worker.platform.process_journal import ProcessJournal

                journal = path.parent / "processes.json"
                confirmed = journal.exists() and ProcessJournal.reconcile(journal)
                state["state"] = "lost" if confirmed else "recovery_required"
                state["cleanup_confirmed"] = bool(confirmed)
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
                kind = "debug" if method == "debug.start" else "interact"
                path = self.root / kind / result["session_id"] / "session.json"
                state = load_json(path) if path.exists() else {}
                if result["session_id"] not in registry or state.get("state") != "running":
                    observed = state.get("state", "recovery_required")
                    if observed == "lost" and not state.get("cleanup_confirmed"):
                        observed = "recovery_required"
                    return {
                        **result,
                        "state": observed,
                        "cleanup_confirmed": state.get("cleanup_confirmed", observed == "closed"),
                        "reason": "session ownership reconciled",
                    }
        return result

    def close_all(self):
        failures = []
        for registry in [self.debug, self.interact]:
            for session in registry.values():
                try:
                    session.close()
                except Exception as error:
                    failures.append(error)
        if failures:
            raise failures[0]

    def _close_absent(self, kind, params):
        identifier = params.get("session_id", "")
        if not re.fullmatch(r"[0-9a-f]{32}", identifier):
            raise WorkerError("session_lost", "session has no verifiable ownership record")
        directory = self.root / kind / identifier
        metadata = directory / "session.json"
        if not metadata.exists():
            raise WorkerError("outcome_unknown", "session cleanup ownership record is missing")
        state = load_json(metadata)
        if state["state"] == "closed":
            return {"session_id": identifier, "state": "closed", "cleanup_confirmed": True}
        from wukong_worker.platform.process_journal import ProcessJournal

        journal = directory / "processes.json"
        confirmed = journal.exists() and ProcessJournal.reconcile(journal)
        state.update(
            state="lost" if confirmed else "recovery_required", cleanup_confirmed=bool(confirmed)
        )
        atomic_json(metadata, state)
        if not confirmed:
            raise WorkerError(
                "outcome_unknown",
                "session cleanup remains unconfirmed; reconcile its owning environment",
            )
        return {"session_id": identifier, "state": "lost", "cleanup_confirmed": True}

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
        from wukong_worker.analysis.profile import process_profile

        with session.lock:
            if session.process.poll() is not None:
                raise ValueError("interactive process exited")
            return {"session_id": session.id, "process": process_profile(session.process.pid)}

    def interact_close(self, params):
        with self.lock:
            session = self.interact.get(params.get("session_id"))
        if session is None:
            return self._close_absent("interact", params)
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
        from wukong_worker.debugger.crash import debugger_snapshot

        return debugger_snapshot(session)

    def debug_profile(self, params):
        session = self._debug(params)
        return session.profile()

    def debug_close(self, params):
        with self.lock:
            session = self.debug.get(params.get("session_id"))
        if session is None:
            return self._close_absent("debug", params)
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
