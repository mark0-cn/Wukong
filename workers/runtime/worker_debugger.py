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
from worker_common import atomic_json, confined, safe_env, sha256_file
from worker_processes import detached_options, terminate_tree

try:  # GDB/MI parsing is optional: the bridge must serve jobs without it.
    from pygdbmi.gdbmiparser import parse_response as parse_mi_response
except ImportError:
    parse_mi_response = None

def load_launch_environment(root,params):
    from worker_launch import load
    spec,_,_,_,_=load(root,params)
    return spec.get("env",{})

class DebugSession:
    """One lock and one result-token queue own each GDB/MI process."""
    engine = "gdb_mi"
    def __init__(self, root, params, isolated=False):
        self.id = uuid.uuid4().hex
        self.directory = root / "debug" / self.id
        self.directory.mkdir(parents=True)
        self.lock = threading.RLock()
        self.stop_condition = threading.Condition()
        self.stop_sequence = 0
        self.last_stop = None
        self.records = queue.Queue(maxsize=2048)
        self.events = collections.deque(maxlen=256)
        self.token = 0
        self.dead = False
        self.transcript_bytes = 0
        target = confined(root, params["target"], True)
        self.mode = params.get("mode", "launch")
        if self.mode not in {"launch", "dump"}:
            raise ValueError("invalid debugger mode")
        self.target_sha256 = sha256_file(target)
        dump = confined(root, params["dump"], True) if self.mode == "dump" else None
        self.dump_sha256 = sha256_file(dump) if dump else None
        self.execution_state = "stopped" if dump else "loaded"
        self.isolated = isolated
        self.experiment = None
        args = list(params.get("args", []))
        self.stdin_path = confined(root, params["_stdin"], True) if params.get("_stdin") else None
        launch = None
        if dump:
            if args or params.get("launch_spec") or params.get("input"):
                raise ValueError("dump inspection cannot launch a process or supply input")
        elif params.get("launch_spec"):
            from worker_launch import resolve
            launch=resolve(root,params,isolated,self.stdin_path or (confined(root,params["input"],True) if params.get("input") else None))
            if launch["input_mode"]=="pty": raise ValueError("PTY debugging needs an external terminal; use interactive PTY or a debug container")
            target,args,self.experiment=launch["target"],launch["args"],launch["experiment"]
            self.stdin_path=launch["stdin_path"] if launch["input_mode"]=="stdin_pipe" else None
        elif isolated:
            import isolation
            self.experiment = isolation.sandbox(root, isolation.allocate_uid(root))
            target = isolation.stage_file(target, self.experiment, "target.bin", True)
            for index, argument in enumerate(args):
                path = Path(argument)
                source = path.resolve() if path.is_absolute() else (root / path).resolve()
                if root in source.parents and source.is_file():
                    args[index] = str(isolation.stage_file(source, self.experiment))
            if self.stdin_path:
                self.stdin_path = isolation.stage_file(self.stdin_path, self.experiment, "stdin.bin")
            isolation.seal(self.experiment)
        launch_args = ["--exec", str(target), "--core", str(dump)] if dump else ["--args", str(target)] + args
        self.process = subprocess.Popen(["gdb", "--quiet", "--nx", "--nh", "-iex", "set auto-load off", "--interpreter=mi2"] + launch_args,
                                        cwd=self.directory, env=safe_env(params.get("env")), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", bufsize=1, **detached_options())
        threading.Thread(target=self.read_records, daemon=True).start()
        self.command("-gdb-set pagination off", 10, True)
        self.command("-gdb-set mi-async on", 10, True)
        self.command("-gdb-set disable-randomization off", 10, True)
        if launch:
            import shlex
            wrapper=[]
            if self.experiment:
                uid=str(self.experiment["uid"])
                wrapper=["setpriv","--reuid="+uid,"--regid="+uid,"--clear-groups","--no-new-privs"]
            if launch["loader"]: wrapper += [str(launch["loader"]),"--library-path",str(launch["library_path"])]
            if wrapper: self.command("-gdb-set exec-wrapper " + shlex.join(wrapper),10,True)
            self.command("-environment-cd " + json.dumps(str(launch["cwd"])),10,True)
            for key,value in load_launch_environment(root,params).items():
                self.command("-gdb-set environment " + json.dumps(key+"="+value),10,True)
        elif self.experiment:
            uid = str(self.experiment["uid"])
            self.command("-gdb-set exec-wrapper setpriv --reuid=" + uid + " --regid=" + uid + " --clear-groups --no-new-privs", 10, True)
            self.command("-environment-cd " + json.dumps(self.experiment["work"]), 10, True)
        self.launch_identity=launch["identity"] if launch else None
        self.persist("running")

    def persist(self, state):
        atomic_json(self.directory / "session.json", {"session_id": self.id, "state": state, "gdb_pid": self.process.pid,
                    "daemon_pid": os.getpid(), "launch_identity":self.launch_identity, "updated_at": time.time()})

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
                if parsed.get("message") == "running":
                    self.execution_state = "running"
                elif parsed.get("message") == "stopped":
                    with self.stop_condition:
                        self.execution_state = "exited" if (parsed.get("payload") or {}).get("reason", "").startswith("exited") else "stopped"
                        self.last_stop = parsed.get("payload")
                        self.stop_sequence += 1
                        self.stop_condition.notify_all()
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
        if not internal and (self.isolated or getattr(self, "mode", "launch") == "dump"):
            from worker_gdb_policy import permitted_command
            command = permitted_command(command, dump=getattr(self, "mode", "launch") == "dump")
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

    def profile(self):
        from worker_profile import process_profile
        with self.lock:
            groups = self.command("-list-thread-groups", 10, True)
            candidates = [g for g in (groups.get("payload") or {}).get("groups", []) if g.get("pid")]
            if len(candidates) != 1: raise ValueError("runtime profile requires exactly one live inferior")
            return {"session_id":self.id, "launch_identity":self.launch_identity, "process":process_profile(candidates[0]["pid"]),
                    "transcript":"debug/" + self.id + "/transcript.mi"}

    def inspect(self, params, timeout):
        from worker_debug_operations import gdb_inspect
        return gdb_inspect(self, params, timeout)

    def control(self, params, timeout):
        from worker_debug_operations import gdb_control
        return gdb_control(self, params, timeout)

    def close(self):
        with self.lock:
            self.dead = True
            terminate_tree(self.process)
            self.persist("closed")
        return {"session_id": self.id, "state": "closed"}
