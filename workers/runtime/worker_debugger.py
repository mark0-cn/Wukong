"""Debugger."""
from pathlib import Path
import collections
import json
import os
import queue
import re
import subprocess
import threading
import time
import uuid
from worker_common import atomic_json, confined, safe_env
from worker_processes import detached_options, terminate_tree

try:  # GDB/MI parsing is optional: the bridge must serve jobs without it.
    from pygdbmi.gdbmiparser import parse_response as parse_mi_response
except ImportError:
    parse_mi_response = None

class DebugSession:
    """One lock and one result-token queue own each GDB/MI process."""
    def __init__(self, root, params, isolated=False):
        self.id = uuid.uuid4().hex
        self.directory = root / "debug" / self.id
        self.directory.mkdir(parents=True)
        self.lock = threading.Lock()
        self.records = queue.Queue(maxsize=2048)
        self.events = collections.deque(maxlen=256)
        self.token = 0
        self.dead = False
        self.transcript_bytes = 0
        target = confined(root, params["target"], True)
        self.isolated = isolated
        self.experiment = None
        args = list(params.get("args", []))
        if isolated:
            import isolation
            self.experiment = isolation.sandbox(root, isolation.allocate_uid(root))
            target = isolation.stage_file(target, self.experiment, "target.bin", True)
            for index, argument in enumerate(args):
                path = Path(argument)
                source = path.resolve() if path.is_absolute() else (root / path).resolve()
                if root in source.parents and source.is_file():
                    args[index] = str(isolation.stage_file(source, self.experiment))
            isolation.seal(self.experiment)
        self.process = subprocess.Popen(["gdb", "--quiet", "--nx", "--nh", "-iex", "set auto-load off", "--interpreter=mi2", "--args", str(target)] + args,
                                        cwd=self.directory, env=safe_env(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", bufsize=1, **detached_options())
        threading.Thread(target=self.read_records, daemon=True).start()
        self.command("-gdb-set pagination off", 10, True)
        self.command("-gdb-set mi-async on", 10, True)
        if self.experiment:
            uid = str(self.experiment["uid"])
            self.command("-gdb-set exec-wrapper setpriv --reuid=" + uid + " --regid=" + uid + " --clear-groups --no-new-privs", 10, True)
            self.command("-environment-cd " + json.dumps(self.experiment["work"]), 10, True)
        self.persist("running")

    def persist(self, state):
        atomic_json(self.directory / "session.json", {"session_id": self.id, "state": state, "gdb_pid": self.process.pid,
                    "daemon_pid": os.getpid(), "updated_at": time.time()})

    def read_records(self):
        with (self.directory / "transcript.mi").open("w", encoding="utf-8") as transcript:
            for raw in iter(lambda: self.process.stdout.readline(65537), ""):
                if len(raw) > 65536:
                    self.dead = True
                    terminate_tree(self.process)
                    break
                line = raw.rstrip("\r\n")
                if self.transcript_bytes < 4 * 1024 * 1024:
                    transcript.write(line + "\n")
                    transcript.flush()
                    self.transcript_bytes += len(line.encode("utf-8"))
                try:
                    if parse_mi_response is None:
                        raise ValueError("pygdbmi is not installed")
                    parsed = parse_mi_response(line)
                except Exception:
                    self.dead = True
                    terminate_tree(self.process)
                    break
                record = {**parsed, "record": line}
                if parsed["type"] == "result" and parsed.get("token") is not None:
                    try:
                        self.records.put_nowait(record)
                    except queue.Full:
                        self.dead = True
                        terminate_tree(self.process)
                        break
                elif line and parsed["type"] != "done":
                    self.events.append(record)
        self.dead = True

    def command(self, command, timeout, internal=False):
        if not re.fullmatch(r"-[a-z][a-z0-9-]*(?: [^\r\n\0]*)?", command) or len(command) > 32768:
            raise ValueError("one GDB/MI command is required; raw CLI and multiline commands are rejected")
        if self.isolated and not internal:
            allowed = {"break-insert", "break-delete", "break-enable", "break-disable", "break-condition", "break-after", "break-list", "exec-run", "exec-continue", "exec-next", "exec-next-instruction", "exec-step", "exec-step-instruction", "exec-finish", "exec-until", "exec-interrupt", "stack-list-frames", "stack-list-arguments", "stack-list-locals", "stack-list-variables", "stack-info-frame", "stack-select-frame", "thread-info", "thread-select", "data-evaluate-expression", "data-read-memory-bytes", "data-write-memory-bytes", "data-list-register-names", "data-list-register-values", "data-disassemble", "symbol-info-functions", "symbol-info-variables"}
            if command.split(" ", 1)[0][1:] not in allowed:
                raise ValueError("GDB/MI operation is not permitted by isolated debugger policy")
        with self.lock:
            if self.dead or self.process.poll() is not None:
                raise ValueError("debug session is closed or lost")
            self.token += 1
            token = str(self.token)
            self.process.stdin.write(token + command + "\n")
            self.process.stdin.flush()
            deadline = time.monotonic() + timeout
            records = []
            while time.monotonic() < deadline:
                try:
                    record = self.records.get(timeout=min(0.1, max(0.01, deadline - time.monotonic())))
                except queue.Empty:
                    if self.dead:
                        raise ValueError("GDB terminated while executing command")
                    continue
                records.append(record)
                if record.get("token") == self.token:
                    return {"session_id": self.id, "token": self.token, "class": record["message"],
                            "record": record["record"], "payload": record.get("payload"),
                            "events": list(self.events)[-64:], "transcript": "debug/" + self.id + "/transcript.mi"}
            # The command may still have effects. Invalidate the session, never
            # issue another command against an unknown debugger state.
            self.dead = True
            terminate_tree(self.process)
            self.persist("lost")
            raise ValueError("GDB command timeout; session invalidated")

    def close(self):
        with self.lock:
            self.dead = True
            terminate_tree(self.process)
            self.persist("closed")
        return {"session_id": self.id, "state": "closed"}
