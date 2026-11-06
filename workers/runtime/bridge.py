#!/usr/bin/env python3
"""Persistent, credential-free runtime RPC bridge (Python standard library only).

The stdio relay may be disconnected without interrupting jobs. A local daemon
owns debugger sessions; each background job has a detached supervisor, durable
metadata, bounded logs, a cancellation file, and a fresh execution directory.
This is a process manager, NOT a sandbox: use Docker/VM/SSH isolation for targets.
"""
import argparse
import base64
import collections
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import re
import secrets
import shutil
import signal
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import uuid

PROTOCOL = 1
MAX_RPC = 48 * 1024 * 1024
MAX_FILE = 32 * 1024 * 1024
TERMINAL = {"completed", "failed", "cancelled", "timed_out", "lost"}
ENV_KEYS = {"PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "PATHEXT", "COMSPEC", "HOME", "USERPROFILE", "LANG", "LC_ALL", "JAVA_HOME", "RE_AGENT_GHIDRA_HEADLESS", "RE_AGENT_GHIDRA_SCRIPT_DIR"}


def safe_env(extra=None):
    # Windows enumerates environment names in upper case; SYSTEMROOT is needed
    # by Winsock even when Python itself launches successfully without it.
    allowed = {key.upper() for key in ENV_KEYS}
    env = {k: v for k, v in os.environ.items() if k.upper() in allowed}
    env.update({"LANG": "C.UTF-8", "PYTHONUNBUFFERED": "1"})
    for k, v in (extra or {}).items():
        if not re.fullmatch(r"(?:AFL_[A-Z0-9_]+|QEMU_[A-Z0-9_]+|ASAN_OPTIONS|UBSAN_OPTIONS)", k):
            raise ValueError("environment variable not permitted: " + k)
        if len(str(v)) > 4096:
            raise ValueError("environment value too long")
        env[k] = str(v)
    return env


def atomic_json(path, data):
    path = Path(path)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    os.replace(temporary, path)


def load_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def confined(root, relative, exists=False):
    root = Path(root).resolve()
    value = Path(str(relative))
    if value.is_absolute() or value.drive or ".." in value.parts:
        raise ValueError("path must be relative to the worker state directory")
    value = (root / value).resolve()
    if value != root and root not in value.parents:
        raise ValueError("path escapes worker state directory")
    if exists and not value.exists():
        raise ValueError("worker path does not exist: " + str(relative))
    return value


def job_dir(root, job_id):
    if not re.fullmatch(r"[0-9a-f]{32}", str(job_id)):
        raise ValueError("invalid job_id")
    return confined(root, "jobs/" + job_id, True)


def detached_options():
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW}
    return {"start_new_session": True}


def attach_windows_job(process):
    """A kill-on-close Job Object owns descendants, including orphaned children."""
    if os.name != "nt":
        return
    import ctypes
    from ctypes import wintypes
    class BasicLimits(ctypes.Structure):
        _fields_ = [("ProcessTime", ctypes.c_longlong), ("JobTime", ctypes.c_longlong), ("Flags", wintypes.DWORD),
                    ("MinimumWorkingSet", ctypes.c_size_t), ("MaximumWorkingSet", ctypes.c_size_t), ("ActiveProcesses", wintypes.DWORD),
                    ("Affinity", ctypes.c_size_t), ("Priority", wintypes.DWORD), ("Scheduling", wintypes.DWORD)]
    class IoCounters(ctypes.Structure):
        _fields_ = [(name, ctypes.c_ulonglong) for name in ["ReadOps", "WriteOps", "OtherOps", "ReadBytes", "WriteBytes", "OtherBytes"]]
    class ExtendedLimits(ctypes.Structure):
        _fields_ = [("Basic", BasicLimits), ("Io", IoCounters), ("ProcessMemory", ctypes.c_size_t), ("JobMemory", ctypes.c_size_t),
                    ("PeakProcessMemory", ctypes.c_size_t), ("PeakJobMemory", ctypes.c_size_t)]
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
    kernel.CreateJobObjectW.restype = wintypes.HANDLE
    kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
    kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.CreateJobObjectW(None, None)
    limits = ExtendedLimits()
    limits.Basic.Flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if handle and kernel.SetInformationJobObject(handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)) and kernel.AssignProcessToJobObject(handle, int(process._handle)):
        process._re_agent_job = (kernel, handle)
    elif handle:
        kernel.CloseHandle(handle)


def close_windows_job(process):
    job = getattr(process, "_re_agent_job", None)
    if job:
        job[0].CloseHandle(job[1])
        process._re_agent_job = None
        return True
    return False


def process_identity(pid):
    """Creation identity, not PID alone. Unknown access is not proof of death."""
    if not pid:
        return "unknown"
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.OpenProcess(0x1000, False, int(pid))
        if not handle:
            return "missing" if ctypes.get_last_error() == 87 else "unknown"
        try:
            times = [wintypes.FILETIME() for _ in range(4)]
            if kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                return "windows:" + str((times[0].dwHighDateTime << 32) | times[0].dwLowDateTime)
            return "unknown"
        finally:
            kernel.CloseHandle(handle)
    if sys.platform.startswith("linux"):
        try:
            stat = Path("/proc") / str(int(pid)) / "stat"
            fields = stat.read_text().rsplit(")", 1)[1].split()
            if fields[0] == "Z":
                return "missing"
            return "linux:" + fields[19]
        except FileNotFoundError:
            return "missing"
        except (OSError, IndexError):
            return "unknown"
    try:
        os.kill(int(pid), 0)
        return "unknown"
    except ProcessLookupError:
        return "missing"
    except PermissionError:
        return "unknown"


def terminate_tree(process):
    if close_windows_job(process):
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        return
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=1.5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()


def bounded_int(value, default, minimum, maximum):
    value = int(value if value is not None else default)
    if not minimum <= value <= maximum:
        raise ValueError("numeric parameter outside allowed range")
    return value


def tail(path, limit=8192):
    if not Path(path).is_file():
        return ""
    with open(path, "rb") as stream:
        stream.seek(max(0, Path(path).stat().st_size - limit))
        return stream.read(limit).decode("utf-8", errors="replace")


def artifact(root, path):
    path = Path(path).resolve()
    if root != path and root not in path.parents:
        raise ValueError("artifact outside worker root")
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return {"path": path.relative_to(root).as_posix(), "sha256": digest.hexdigest(), "size": path.stat().st_size}


def public_file(root, relative):
    path = confined(root, relative, True)
    parts = path.relative_to(root).parts
    if not parts or parts[0] not in {"files", "jobs", "debug", "fuzz", "ghidra", "interact"}:
        raise ValueError("path is private worker metadata")
    if parts[0] == "ghidra" and (len(parts) < 2 or parts[1] != "results"):
        raise ValueError("path is private Ghidra project metadata")
    if path.name in {"spec.json", "status.json", "heartbeat.json", "cancel", "session.json", "supervisor.log"}:
        raise ValueError("path is private worker metadata; use status APIs")
    return path


def run_process(argv, cwd, env, timeout_secs, output_limit, log_prefix, cancel_file=None, experiment=None):
    if not isinstance(argv, list) or not argv or len(argv) > 128 or any(not isinstance(a, str) or "\0" in a or len(a) > 65536 for a in argv):
        raise ValueError("argv must be an array of 1..128 bounded strings")
    start = time.monotonic()
    credential_options = {}
    if experiment:
        import isolation
        credential_options = isolation.credentials(experiment)
        env = {**env, "HOME": experiment["work"], "TMPDIR": experiment["work"]}
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, **detached_options(), **credential_options)
    attach_windows_job(process)
    counts = {"stdout": 0, "stderr": 0}

    def drain(name, stream):
        with open(str(log_prefix) + "." + name, "wb") as log:
            while True:
                data = stream.read(8192)
                if not data:
                    break
                remaining = max(0, output_limit - counts[name])
                if remaining:
                    log.write(data[:remaining])
                counts[name] += len(data)
        stream.close()

    readers = [threading.Thread(target=drain, args=(name, getattr(process, name)), daemon=True) for name in counts]
    for thread in readers:
        thread.start()
    state = "completed"
    while process.poll() is None:
        if cancel_file and Path(cancel_file).exists():
            state = "cancelled"
            terminate_tree(process)
            break
        if time.monotonic() - start >= timeout_secs:
            state = "timed_out"
            terminate_tree(process)
            break
        time.sleep(0.03)
    # A completed parent must not leave descendants holding output pipes open.
    if os.name == "nt":
        close_windows_job(process)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    for thread in readers:
        thread.join(timeout=5)
    return {"state": state, "exit_code": process.returncode,
            "signal": -process.returncode if process.returncode is not None and process.returncode < 0 and os.name != "nt" else None,
            "duration_secs": round(time.monotonic() - start, 4),
            "stdout": tail(str(log_prefix) + ".stdout"), "stderr": tail(str(log_prefix) + ".stderr"),
            "output_bytes": counts, "output_truncated": any(v > output_limit for v in counts.values())}


def crash_signature(result):
    code = result.get("exit_code")
    if result.get("state") != "completed":
        return None
    if result.get("signal"):
        return "signal:" + str(result["signal"])
    if os.name == "nt" and code is not None and (code & 0xFFFFFFFF) in {0xC0000005, 0xC000001D, 0xC0000094, 0xC00000FD, 0xC0000409}:
        return "exception:" + hex(code & 0xFFFFFFFF)
    return None


def expectation_matches(result, expected):
    if result["state"] != "completed":
        return False
    if not expected:
        return crash_signature(result) is not None
    allowed = {"exit_code", "signal", "stdout_contains", "stderr_contains"}
    if set(expected) - allowed:
        raise ValueError("unsupported verification expectation")
    return all((str(value) in result[key[:-9]]) if key.endswith("_contains") else result.get(key) == value
               for key, value in expected.items())


def replay(root, directory, spec, input_bytes, index):
    target = confined(root, spec["target"], True)
    sandbox = directory / ("trial-%04d" % index)
    sandbox.mkdir()
    input_path = sandbox / "input.bin"
    input_path.write_bytes(input_bytes)
    input_path.chmod(0o444)
    experiment = None
    cwd = sandbox
    if spec.get("isolated"):
        import isolation
        experiment = isolation.sandbox(root, isolation.allocate_uid(root))
        target = isolation.stage_file(target, experiment, "target.bin", True)
        public_input = Path(experiment["readonly"]) / "input.bin"
        public_input.write_bytes(input_bytes)
        public_input.chmod(0o444)
        input_path = public_input
        isolation.seal(experiment)
        cwd = Path(experiment["work"])
    argv = [str(target)] + [str(input_path) if arg == "@@" else str(arg) for arg in spec.get("args", ["@@"])]
    result = run_process(argv, cwd, safe_env(spec.get("env")), spec["timeout_secs"], spec["output_limit"], sandbox / "output", directory / "cancel", experiment)
    result["experiment_uid"] = experiment["uid"] if experiment else None
    result["metadata_protected"] = bool(experiment)
    result["input_sha256"] = hashlib.sha256(input_bytes).hexdigest()
    result["crash_signature"] = crash_signature(result)
    result["trial"] = index
    return result


def supervise_job(root, directory):
    spec = load_json(directory / "spec.json")
    meta = load_json(directory / "status.json")
    token = spec["runner_token"]
    meta.update({"state": "running", "supervisor_pid": os.getpid(), "supervisor_identity": process_identity(os.getpid()), "started_at": time.time(), "runner_token": token})
    atomic_json(directory / "status.json", meta)
    stop = threading.Event()

    def heartbeat():
        while not stop.wait(0.5):
            atomic_json(directory / "heartbeat.json", {"runner_token": token, "pid": os.getpid(), "at": time.time()})

    atomic_json(directory / "heartbeat.json", {"runner_token": token, "pid": os.getpid(), "at": time.time()})
    threading.Thread(target=heartbeat, daemon=True).start()
    try:
        kind = spec["kind"]
        if kind in {"process", "fuzz"}:
            cwd = confined(root, spec.get("cwd", "."), True)
            argv = spec["argv"]
            experiment = None
            if spec.get("isolated"):
                import isolation
                experiment = isolation.sandbox(root, spec["execution_uid"])
                cwd = Path(experiment["work"])
                if kind == "fuzz":
                    argv = list(argv)
                    target_index = argv.index("--") + 1
                    argv[target_index] = str(isolation.stage_file(argv[target_index], experiment, "target.bin", True))
                    argv[argv.index("-i") + 1] = str(isolation.stage_directory(argv[argv.index("-i") + 1], experiment, "seeds"))
                    argv[argv.index("-o") + 1] = str(cwd / "fuzz")
                else:
                    staged = []
                    for argument in argv:
                        path = Path(argument)
                        absolute = path.resolve() if path.is_absolute() else (root / path).resolve()
                        relative_input = not path.is_absolute() and argument.replace("\\", "/").split("/", 1)[0] in {"files", "jobs", "fuzz"}
                        if (path.is_absolute() and root in absolute.parents) or relative_input:
                            if absolute.is_file():
                                argument = str(isolation.stage_file(absolute, experiment, executable=(len(staged) == 0)))
                            elif absolute.is_dir():
                                argument = str(isolation.stage_directory(absolute, experiment, "directory-" + str(len(staged))))
                            else:
                                raise ValueError("isolated jobs must write output relative to their fresh working directory")
                        staged.append(argument)
                    argv = staged
                isolation.seal(experiment)
                atomic_json(directory / "experiment.json", experiment)
            result = run_process(argv, cwd, safe_env(spec.get("env")), spec["timeout_secs"], spec["output_limit"], directory / "output", directory / "cancel", experiment)
            result["experiment_uid"] = experiment["uid"] if experiment else None
            result["metadata_protected"] = bool(experiment)
            if experiment and kind == "process":
                import isolation
                try:
                    _, paths = isolation.collect(root, experiment, directory / "artifacts", "all", safe_env())
                    result["artifacts"] = [artifact(root, path) for path in paths]
                except Exception as error:
                    result["artifact_collection_error"] = str(error)[:1024]
            meta["state"] = result["state"] if result["state"] != "completed" or result["exit_code"] == 0 else "failed"
        elif kind in {"replay", "verify"}:
            input_bytes = confined(root, spec["input"], True).read_bytes()
            results = []
            for index in range(spec.get("trials", 1)):
                if (directory / "cancel").exists():
                    break
                results.append(replay(root, directory, spec, input_bytes, index))
            successes = sum(expectation_matches(r, spec.get("expected")) for r in results)
            result = {"trials": len(results), "attempts": len(results), "requested_trials": spec.get("trials", 1), "successes": successes,
                      "passed": bool(results) and len(results) == spec.get("trials", 1) and successes == len(results),
                      "observed_stage": "crash" if results and all(r["crash_signature"] for r in results) else "behavior",
                      "results": results, "verifier_agent_id": spec.get("verifier_agent_id"),
                      "producer_agent_id": spec.get("producer_agent_id"), "call_id": spec.get("call_id"),
                      "target": artifact(root, confined(root, spec["target"], True)),
                      "input": artifact(root, confined(root, spec["input"], True)),
                      "expected": spec.get("expected"), "environment": {"platform": platform.platform(), "python": platform.python_version()},
                      "fresh_working_directory_per_trial": True}
            meta["state"] = "cancelled" if (directory / "cancel").exists() else "completed"
        elif kind == "minimize":
            data = confined(root, spec["input"], True).read_bytes()
            baseline = replay(root, directory, spec, data, 0)
            signature = crash_signature(baseline)
            if not signature:
                raise ValueError("input does not reproduce a crash; minimization requires a stable crash signature")
            attempts, granularity, index = 1, 2, 1
            deadline = time.monotonic() + spec["minimize_timeout_secs"]
            while len(data) > 1 and attempts < spec["max_attempts"] and time.monotonic() < deadline and not (directory / "cancel").exists():
                chunk = max(1, (len(data) + granularity - 1) // granularity)
                reduced = False
                for offset in range(0, len(data), chunk):
                    if attempts >= spec["max_attempts"] or time.monotonic() >= deadline or (directory / "cancel").exists():
                        break
                    candidate = data[:offset] + data[offset + chunk:]
                    observation = replay(root, directory, spec, candidate, index)
                    attempts += 1
                    index += 1
                    if crash_signature(observation) == signature:
                        data, granularity, reduced = candidate, max(2, granularity - 1), True
                        break
                if not reduced:
                    if granularity >= len(data):
                        break
                    granularity = min(len(data), granularity * 2)
            minimized = directory / "minimized.bin"
            minimized.write_bytes(data)
            result = {"artifact": artifact(root, minimized), "original_size": confined(root, spec["input"], True).stat().st_size,
                      "minimized_size": len(data), "attempts": attempts, "crash_signature": signature,
                      "complete": attempts < spec["max_attempts"] and time.monotonic() < deadline and not (directory / "cancel").exists()}
            meta["state"] = "cancelled" if (directory / "cancel").exists() else "completed"
        else:
            raise ValueError("unknown job kind")
        meta["result"] = result
    except Exception as error:
        meta.update({"state": "failed", "error": str(error)[:4096]})
    finally:
        stop.set()
        meta["finished_at"] = time.time()
        atomic_json(directory / "status.json", meta)


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
            for raw in self.process.stdout:
                line = raw.rstrip("\r\n")[:65536]
                if self.transcript_bytes < 4 * 1024 * 1024:
                    transcript.write(line + "\n")
                    transcript.flush()
                    self.transcript_bytes += len(line.encode("utf-8"))
                if re.match(r"^\d+\^", line):
                    try:
                        self.records.put_nowait(line)
                    except queue.Full:
                        self.dead = True
                        terminate_tree(self.process)
                        break
                elif line and line != "(gdb) ":
                    self.events.append(line)
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
                    line = self.records.get(timeout=min(0.1, max(0.01, deadline - time.monotonic())))
                except queue.Empty:
                    if self.dead:
                        raise ValueError("GDB terminated while executing command")
                    continue
                records.append(line)
                if line.startswith(token + "^"):
                    result_class = line[len(token) + 1:].split(",", 1)[0]
                    return {"session_id": self.id, "token": self.token, "class": result_class, "record": line,
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


class InteractSession:
    """A persistent interactive process: bounded stdin writes and stdout reads.

    One agent owns a session and all of its writes are serialized. Output is
    captured into a bounded rolling buffer; reads return the newest window.
    """

    def __init__(self, root, params, isolated=False):
        self.id = uuid.uuid4().hex
        self.directory = root / "interact" / self.id
        self.directory.mkdir(parents=True)
        self.lock = threading.Lock()
        self.dead = False
        self.buffer = bytearray()
        self.total_bytes = 0
        self.last_data = 0.0
        self.experiment = None
        argv = list(params["argv"])
        if not argv or not all(isinstance(argument, str) for argument in argv):
            raise ValueError("interactive argv must be a nonempty list of strings")
        self.isolated = isolated
        if isolated:
            import isolation
            self.experiment = isolation.sandbox(root, isolation.allocate_uid(root))
            for index, argument in enumerate(argv):
                path = Path(argument)
                absolute = path.resolve() if path.is_absolute() else (root / path).resolve()
                relative_input = not path.is_absolute() and argument.replace("\\", "/").split("/", 1)[0] in {"files", "jobs", "fuzz"}
                if (path.is_absolute() and root in absolute.parents) or relative_input:
                    if absolute.is_file():
                        argv[index] = str(isolation.stage_file(absolute, self.experiment, executable=(index == 0)))
                    elif absolute.is_dir():
                        argv[index] = str(isolation.stage_directory(absolute, self.experiment, "directory-" + str(index)))
                    else:
                        raise ValueError("isolated interactive argv paths must exist under the state directory")
            isolation.seal(self.experiment)
        cwd = Path(self.experiment["work"]) if self.experiment else self.directory
        self.process = subprocess.Popen(argv, cwd=cwd, env=safe_env(params.get("env")),
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, bufsize=0, **detached_options())
        threading.Thread(target=self.drain, daemon=True).start()
        self.persist("running")

    def persist(self, state):
        atomic_json(self.directory / "session.json", {
            "session_id": self.id, "state": state, "pid": self.process.pid,
            "daemon_pid": os.getpid(), "updated_at": time.time()})

    def drain(self):
        while True:
            chunk = self.process.stdout.read(4096)
            if not chunk:
                break
            with self.lock:
                self.buffer.extend(chunk)
                self.total_bytes += len(chunk)
                if len(self.buffer) > 1024 * 1024:
                    del self.buffer[:len(self.buffer) - 1024 * 1024]
                self.last_data = time.time()
        with self.lock:
            self.dead = True
        self.persist("exited")

    def write(self, data, read_timeout_secs, quiet_ms=250):
        with self.lock:
            if self.dead or self.process.poll() is not None:
                return self.snapshot("exited", data_sent=False)
            self.process.stdin.write(data)
            self.process.stdin.flush()
            self.last_data = time.time()
        deadline = time.monotonic() + read_timeout_secs
        while time.monotonic() < deadline:
            with self.lock:
                exited = self.process.poll() is not None
                quiet = time.time() - self.last_data >= quiet_ms / 1000.0
            if exited or quiet:
                return self.snapshot("exited" if exited else "running")
            time.sleep(0.02)
        return self.snapshot("running", timed_out=True)

    def snapshot(self, state, data_sent=True, timed_out=False):
        with self.lock:
            captured = bytes(self.buffer)
            self.buffer.clear()
        text = captured.decode("utf-8", errors="replace")
        return {"session_id": self.id, "state": state, "captured": text, "captured_bytes": len(captured),
                "truncated": len(text) > 65536, "total_bytes": self.total_bytes,
                "exit_code": self.process.poll(), "timed_out": timed_out, "data_sent": data_sent}

    def close(self):
        with self.lock:
            self.dead = True
            terminate_tree(self.process)
        self.persist("closed")
        return {"session_id": self.id, "state": "closed"}


class Runtime:
    def __init__(self, root, trusted):
        self.root, self.trusted = root.resolve(), trusted
        self.isolated = sys.platform.startswith("linux") and os.geteuid() == 0 and not trusted
        self.debug = {}
        self.interact = {}
        self.lock = threading.RLock()
        (root / "jobs").mkdir(exist_ok=True)
        (root / "files").mkdir(exist_ok=True)
        (root / "operations").mkdir(exist_ok=True)
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

    def doctor(self):
        tools = {}
        for name in ["gdb", "afl-fuzz", "afl-qemu-trace", "afl-qemu-bridge", "afl-tmin", "gcc"]:
            executable = shutil.which(name)
            version = None
            if executable and name in {"gdb", "gcc"}:
                try:
                    version = subprocess.run([executable, "--version"], env=safe_env(), capture_output=True, timeout=5).stdout.decode(errors="replace").splitlines()[0][:256]
                except (OSError, subprocess.TimeoutExpired, IndexError):
                    pass
            tools[name] = {"available": bool(executable), "path": executable, "version": version}
        linux = sys.platform.startswith("linux")
        core_pattern = None
        if linux:
            try:
                core_pattern = Path("/proc/sys/kernel/core_pattern").read_text().strip()[:512]
            except OSError:
                pass
        return {"protocol": PROTOCOL, "platform": platform.platform(), "python": platform.python_version(),
                "worker_root": str(self.root), "isolation": "trusted_local" if self.trusted else "external_worker",
                "core_pattern": core_pattern, "external_core_handler": bool(core_pattern and core_pattern.startswith("|")),
                "metadata_protected_from_targets": self.isolated, "unique_experiment_uids": self.isolated,
                "tools": tools, "capabilities": {"process_jobs": True, "persistent_jobs": True,
                "debug_gdb_mi": tools["gdb"]["available"], "fuzz_qemu": linux and tools["afl-fuzz"]["available"] and (tools["afl-qemu-trace"]["available"] or tools["afl-qemu-bridge"]["available"]),
                "crash_replay": True, "input_minimize": True, "poc_verify": True,
                "formats": ["ELF"] if linux else [], "native_target_execution": True}}

    def start_job(self, params, kind="process"):
        if not self.trusted and not self.isolated:
            raise ValueError("production execution requires a root Linux worker with UID/GID/CHOWN capabilities; use explicit trusted_local only for trusted fixtures")
        spec = dict(params)
        spec["isolated"] = self.isolated
        spec["call_id"] = spec.get("call_id") or spec.get("request_id")
        spec["kind"] = kind
        spec["timeout_secs"] = bounded_int(spec.get("timeout_secs"), 30, 1, 86400)
        spec["output_limit"] = bounded_int(spec.get("output_limit"), 1048576, 1024, 8 * 1048576)
        spec["runner_token"] = secrets.token_hex(32)
        if kind in {"replay", "verify", "minimize"}:
            target = confined(self.root, spec["target"], True)
            input_path = confined(self.root, spec["input"], True)
            if not target.is_file() or not input_path.is_file() or input_path.stat().st_size > MAX_FILE:
                raise ValueError("target/input must be files and input must be <=32 MiB")
            spec["timeout_secs"] = min(spec["timeout_secs"], 60)
            spec["trials"] = bounded_int(spec.get("trials"), 3 if kind == "verify" else 1, 1, 16)
            spec["max_attempts"] = bounded_int(spec.get("max_attempts"), 128, 1, 4096)
            spec["minimize_timeout_secs"] = bounded_int(spec.get("minimize_timeout_secs"), 300, 1, 3600)
            if kind == "verify":
                if not spec.get("verifier_agent_id") or not spec.get("producer_agent_id") or not spec.get("call_id"):
                    raise ValueError("poc.verify requires verifier_agent_id, producer_agent_id and call_id")
                if spec["verifier_agent_id"] == spec["producer_agent_id"]:
                    raise ValueError("independent verifier must differ from PoC producer")
        else:
            if not isinstance(spec.get("argv"), list) or not spec["argv"]:
                raise ValueError("argv is required")
            cwd = confined(self.root, spec.get("cwd", "."), True)
            if not cwd.is_dir():
                raise ValueError("cwd must be a worker directory")
        safe_env(spec.get("env"))
        call_id = spec.get("call_id")
        # Match the complete request, not just an id, to avoid silently reusing
        # unrelated work. Reconnect callers can safely retry job creation.
        fingerprint_spec = {k: v for k, v in spec.items() if k != "runner_token"}
        fingerprint = hashlib.sha256(json.dumps(fingerprint_spec, sort_keys=True).encode()).hexdigest()
        with self.lock:
            if call_id:
                for path in (self.root / "jobs").glob("*/status.json"):
                    existing = load_json(path)
                    if existing.get("call_id") == call_id:
                        if existing["fingerprint"] != fingerprint:
                            raise ValueError("call_id already used with a different request")
                        return self.status(existing["job_id"])
            identifier = hashlib.sha256((kind + ":" + str(spec["request_id"])).encode()).hexdigest()[:32] if spec.get("request_id") else uuid.uuid4().hex
            directory = self.root / "jobs" / identifier
            if directory.exists():
                existing = load_json(directory / "status.json")
                if existing.get("fingerprint") != fingerprint:
                    raise ValueError("request_id already used with different job parameters")
                return self.status(identifier)
            directory.mkdir()
            if self.isolated:
                import isolation
                spec["execution_uid"] = isolation.allocate_uid(self.root)
            meta = {"job_id": identifier, "kind": kind, "state": "queued", "created_at": time.time(), "call_id": call_id,
                    "fingerprint": fingerprint, "runner_token": spec["runner_token"], "artifacts_dir": "jobs/" + identifier}
            atomic_json(directory / "spec.json", spec)
            atomic_json(directory / "status.json", meta)
            with (directory / "supervisor.log").open("ab") as log:
                process = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--state", str(self.root), "--run-job", identifier],
                                           env=safe_env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log, **detached_options())
            # Popen is intentionally not waited: it is owned by its supervisor.
            meta["supervisor_pid"] = process.pid
            return {k: v for k, v in meta.items() if k != "runner_token"}

    def status(self, identifier):
        directory = job_dir(self.root, identifier)
        meta = load_json(directory / "status.json")
        if meta["state"] not in TERMINAL:
            try:
                beat = load_json(directory / "heartbeat.json")
                live = beat["runner_token"] == meta["runner_token"] and time.time() - beat["at"] < 5
            except (OSError, ValueError, KeyError):
                live = time.time() - meta["created_at"] < 10
            if not live:
                current_identity = process_identity(meta.get("supervisor_pid"))
                expected_identity = meta.get("supervisor_identity", "unknown")
                if current_identity == expected_identity and current_identity not in {"missing", "unknown"}:
                    meta["reconciliation"] = "supervisor_alive_heartbeat_stale"
                    live = True
                elif current_identity == "unknown" or (expected_identity == "unknown" and current_identity != "missing"):
                    meta["reconciliation"] = "supervisor_liveness_unconfirmed"
                    live = True
            if not live:
                # Recheck under the shared lock; never restart an ambiguous job.
                with self.lock:
                    fresh = load_json(directory / "status.json")
                    if fresh["state"] not in TERMINAL:
                        fresh.update({"state": "lost", "error": "supervisor process creation identity is absent or changed; effects unknown", "finished_at": time.time()})
                        atomic_json(directory / "status.json", fresh)
                    meta = fresh
        meta.pop("runner_token", None)
        meta["stdout"] = tail(directory / "output.stdout")
        meta["stderr"] = tail(directory / "output.stderr")
        if meta["kind"] == "fuzz":
            spec = load_json(directory / "spec.json")
            out = confined(self.root, spec["afl_output"])
            stats = {}
            isolated_crashes = None
            if spec.get("isolated") and (directory / "experiment.json").is_file():
                import isolation
                try:
                    experiment = load_json(directory / "experiment.json")
                    stats, paths = isolation.collect(self.root, experiment, directory / "fuzz-artifacts", "fuzz", safe_env())
                    isolated_crashes = [artifact(self.root, path) for path in paths]
                except Exception as error:
                    meta["artifact_collection_error"] = str(error)[:1024]
            for path in sorted(out.glob("*/fuzzer_stats"))[:32]:
                stats[path.parent.name] = dict(line.split(":", 1)[0].strip() and [line.split(":", 1)[0].strip(), line.split(":", 1)[1].strip()] for line in path.read_text(errors="replace").splitlines() if ":" in line)
            meta["fuzzer_stats"] = stats
            meta["crashes"] = isolated_crashes if isolated_crashes is not None else [artifact(self.root, path) for path in sorted(out.glob("*/crashes/id*"))[:128] if path.is_file()]
            meta["crash_reporting_override"] = spec.get("env", {}).get("AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES") == "1"
        return meta

    def dispatch(self, method, params):
        effects = {"job.start", "fuzz.start", "debug.start", "debug.command", "debug.close", "file.put", "crash.replay", "input.minimize", "poc.verify", "job.cancel", "fuzz.stop"}
        request_id = params.get("request_id")
        if method not in effects or not request_id:
            return self.dispatch_inner(method, params)
        if not isinstance(request_id, str) or not request_id or len(request_id) > 256:
            raise ValueError("request_id must be a nonempty string <=256 characters")
        fingerprint = hashlib.sha256(json.dumps({"method": method, "params": params}, sort_keys=True).encode()).hexdigest()
        ledger = self.root / "operations" / (hashlib.sha256(request_id.encode()).hexdigest() + ".json")
        # Reservation precedes the action. An owner death cannot cause an
        # uncertain debugger command to be replayed on reconnect.
        with self.lock:
            if ledger.exists():
                operation = load_json(ledger)
                if operation["fingerprint"] != fingerprint:
                    raise ValueError("request_id already used for a different operation")
                if operation["state"] == "completed":
                    result = operation["result"]
                    if method == "debug.start" and result["session_id"] not in self.debug:
                        return {**result, "state": "lost", "reason": "debugger owner daemon restarted"}
                    return result
                if operation["state"] == "failed":
                    raise ValueError(operation["error"])
                if method not in {"job.start", "fuzz.start", "file.put", "crash.replay", "input.minimize", "poc.verify", "job.cancel", "fuzz.stop", "debug.close"}:
                    raise ValueError("prior operation outcome is unknown; reconcile debugger state instead of replaying")
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "pending", "method": method, "at": time.time()})
            try:
                result = self.dispatch_inner(method, params)
            except Exception as error:
                atomic_json(ledger, {"fingerprint": fingerprint, "state": "failed", "error": str(error)[:4096], "at": time.time()})
                raise
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "completed", "result": result, "at": time.time()})
            return result

    def dispatch_inner(self, method, params):
        if method == "ping":
            return {"protocol": PROTOCOL, "daemon_pid": os.getpid(), "state_dir": str(self.root)}
        if method == "doctor":
            return self.doctor()
        if method == "ghidra.query":
            from ghidra_backend import query
            return query(self.root, params)
        if method == "interact.start":
            session = InteractSession(self.root, params, self.isolated)
            self.interact[session.id] = session
            return {"session_id": session.id, "state": "running",
                    "ownership": "single_agent_serialized", "pid": session.process.pid}
        if method in {"interact.write", "interact.close"}:
            session = self.interact.get(params.get("session_id"))
            if session is None:
                raise ValueError("interactive session absent or lost")
            if method == "interact.close":
                self.interact.pop(session.id, None)
                return session.close()
            data = base64.b64decode(params.get("data_base64", ""), validate=True)
            if len(data) > 1024 * 1024:
                raise ValueError("interactive write exceeds 1 MiB")
            return session.write(data, bounded_int(params.get("read_timeout_secs"), 10, 1, 300))
        if method == "angr.solve":
            target = confined(self.root, params["target"], True)
            if not target.is_file():
                raise ValueError("angr target must be a staged file")
            find = params.get("find")
            if not isinstance(find, list) or not find or not all(
                    isinstance(item, int) and 0 <= item < 2**64 for item in find):
                raise ValueError("find must be a nonempty list of module-relative addresses")
            avoid = params.get("avoid") or []
            if not all(isinstance(item, int) and 0 <= item < 2**64 for item in avoid):
                raise ValueError("avoid addresses must be module-relative integers")
            stdin_length = bounded_int(params.get("stdin_length"), 64, 1, 4096)
            max_steps = bounded_int(params.get("max_steps"), 50000, 1, 10_000_000)
            payload = json.dumps({"find": find, "avoid": avoid,
                                  "stdin_length": stdin_length, "max_steps": max_steps,
                                  "printable_only": bool(params.get("printable_only", True))})
            script = str(Path(__file__).with_name("angr_solve.py"))
            # The target travels as an argv entry so isolated jobs stage it
            # into the sandbox like every other worker file reference.
            spec = {"argv": ["python3", script, payload, str(params["target"])],
                    "call_id": params.get("call_id"),
                    "request_id": params.get("request_id"),
                    "timeout_secs": bounded_int(params.get("timeout_secs"), 600, 10, 3600),
                    "output_limit": 1024 * 1024}
            return self.start_job(spec)
        if method == "file.put":
            path = confined(self.root, "files/" + str(params["path"]))
            data = base64.b64decode(params["data_base64"], validate=True)
            if len(data) > MAX_FILE:
                raise ValueError("file upload exceeds 32 MiB")
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists():
                if path.is_file() and path.read_bytes() == data:
                    return artifact(self.root, path)
                raise ValueError("file path already exists with different content; choose another path")
            with path.open("xb") as stream:
                stream.write(data)
            path.chmod(0o555 if params.get("executable") else 0o444)
            return artifact(self.root, path)
        if method == "file.get":
            path = public_file(self.root, params["path"])
            maximum = bounded_int(params.get("max_bytes"), 1048576, 1, MAX_FILE)
            if not path.is_file() or path.stat().st_size > maximum:
                raise ValueError("file exceeds max_bytes or is not a regular file")
            return {**artifact(self.root, path), "data_base64": base64.b64encode(path.read_bytes()).decode()}
        if method == "job.start":
            return self.start_job(params)
        if method in {"job.status", "fuzz.status"}:
            return self.status(params["job_id"])
        if method == "job.list":
            return [self.status(path.parent.name) for path in sorted((self.root / "jobs").glob("*/status.json"))[-256:]]
        if method in {"job.cancel", "fuzz.stop"}:
            directory = job_dir(self.root, params["job_id"])
            (directory / "cancel").touch()
            return self.status(params["job_id"])
        if method == "job.artifacts":
            directory = job_dir(self.root, params["job_id"])
            files = []
            for path in sorted(directory.rglob("*")):
                if path.is_file() and not path.is_symlink() and path.name not in {"spec.json", "status.json", "heartbeat.json", "cancel", "supervisor.log"}:
                    files.append(artifact(self.root, path))
                    if len(files) == 512:
                        break
            return files
        if method == "fuzz.start":
            doctor = self.doctor()
            if not doctor["capabilities"]["fuzz_qemu"]:
                raise ValueError("AFL++ QEMU capability unavailable; inspect doctor before selecting backend")
            target = confined(self.root, params["target"], True)
            seeds = confined(self.root, params["seeds"], True)
            if not seeds.is_dir() or not any(p.is_file() for p in seeds.iterdir()):
                raise ValueError("nonempty seed directory required")
            key = params.get("request_id") or params.get("call_id")
            identifier = hashlib.sha256(("fuzz:" + str(key)).encode()).hexdigest()[:32] if key else uuid.uuid4().hex
            output_relative = "fuzz/" + identifier
            output = confined(self.root, output_relative)
            output.mkdir(parents=True, exist_ok=True)
            seconds = bounded_int(params.get("timeout_secs"), 60, 1, 86400)
            timeout_ms = bounded_int(params.get("exec_timeout_ms"), 1000, 10, 60000)
            memory_mb = bounded_int(params.get("memory_mb"), 256, 16, 65536)
            argv = ["afl-fuzz", "-Q", "-i", str(seeds), "-o", str(output), "-V", str(seconds), "-t", str(timeout_ms), "-m", str(memory_mb), "--", str(target)] + params.get("args", ["@@"])
            spec = {**params, "argv": argv, "afl_output": output_relative, "timeout_secs": seconds + 15,
                    "env": {**params.get("env", {}), "AFL_SKIP_CPUFREQ": "1", "AFL_NO_UI": "1", "AFL_SKIP_CRASHES": "1"}}
            return self.start_job(spec, "fuzz")
        if method in {"crash.replay", "poc.verify", "input.minimize"}:
            return self.start_job(params, {"crash.replay": "replay", "poc.verify": "verify", "input.minimize": "minimize"}[method])
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
                raise ValueError("debug session not owned by this daemon; it is absent or lost")
            if method == "debug.close":
                return session.close()
            if method == "debug.events":
                return {"session_id": session.id, "events": list(session.events), "state": "lost" if session.dead else "running"}
            return session.command(params["command"], bounded_int(params.get("timeout_secs"), 30, 1, 60))
        if method == "worker.shutdown":
            active = [self.status(path.parent.name) for path in (self.root / "jobs").glob("*/status.json")]
            if any(job["state"] not in TERMINAL for job in active):
                raise ValueError("cannot shut down worker while jobs are active; cancel and reconcile them first")
            for session in self.debug.values():
                session.close()
            return {"state": "shutdown"}
        raise ValueError("unsupported worker method: " + method)


def serve(root, trusted):
    runtime = Runtime(root, trusted)
    token = secrets.token_hex(32)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            self.connection.settimeout(310)
            try:
                line = self.rfile.readline(MAX_RPC + 1)
                if len(line) > MAX_RPC:
                    raise ValueError("request exceeds size limit")
                request = json.loads(line)
                if not secrets.compare_digest(str(request.pop("auth", "")), token):
                    raise ValueError("worker authentication failed")
                result = runtime.dispatch(request["method"], request.get("params") or {})
                response = {"jsonrpc": "2.0", "id": request.get("id"), "result": result}
            except Exception as error:
                response = {"jsonrpc": "2.0", "id": locals().get("request", {}).get("id"), "error": {"code": -32000, "message": str(error)[:4096]}}
            encoded = json.dumps(response, ensure_ascii=False).encode() + b"\n"
            if len(encoded) > MAX_RPC:
                encoded = json.dumps({"jsonrpc": "2.0", "id": response.get("id"), "error": {"code": -32001, "message": "response exceeds size limit"}}).encode() + b"\n"
            self.wfile.write(encoded)
            if locals().get("request", {}).get("method") == "worker.shutdown" and "error" not in response:
                threading.Thread(target=self.server.shutdown, daemon=True).start()

    class Server(socketserver.ThreadingTCPServer):
        daemon_threads = True
        allow_reuse_address = False

    with Server(("127.0.0.1", 0), Handler) as server:
        endpoint = {"protocol": PROTOCOL, "host": "127.0.0.1", "port": server.server_address[1], "auth": token, "pid": os.getpid()}
        atomic_json(root / "endpoint.json", endpoint)
        (root / "endpoint.json").chmod(0o600)
        server.serve_forever(poll_interval=0.25)


def relay_call(endpoint, request):
    request = {**request, "auth": endpoint["auth"]}
    with socket.create_connection(("127.0.0.1", int(endpoint["port"])), timeout=5) as connection:
        connection.settimeout(305)
        connection.sendall(json.dumps(request).encode() + b"\n")
        with connection.makefile("rb") as stream:
            line = stream.readline(MAX_RPC + 1)
            if not line or len(line) > MAX_RPC:
                raise ValueError("invalid daemon response")
            return json.loads(line)


def ensure_daemon(root, trusted):
    endpoint_path = root / "endpoint.json"

    def existing():
        try:
            endpoint = load_json(endpoint_path)
            reply = relay_call(endpoint, {"id": 0, "method": "ping"})
            if reply.get("result", {}).get("protocol") == PROTOCOL:
                return endpoint
        except (OSError, ValueError, KeyError, socket.timeout):
            pass
        return None

    endpoint = existing()
    if endpoint:
        return endpoint
    # Serialize daemon startup across relays. A stale lock is only removed after
    # its age exceeds startup allowance and a fresh endpoint probe still fails.
    lock = root / "startup.lock"
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            endpoint = existing()
            if endpoint:
                return endpoint
            if time.time() - lock.stat().st_mtime > 20:
                try:
                    lock.rmdir()
                except OSError:
                    pass
            time.sleep(0.1)
    else:
        raise ValueError("worker daemon startup lock timeout")
    try:
        endpoint = existing()
        if endpoint:
            return endpoint
        argv = [sys.executable, str(Path(__file__).resolve()), "--state", str(root), "--daemon"]
        if trusted:
            argv.append("--trusted-local")
        with (root / "daemon.log").open("ab") as log:
            subprocess.Popen(argv, env=safe_env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log, **detached_options())
        for _ in range(100):
            endpoint = existing()
            if endpoint:
                return endpoint
            time.sleep(0.05)
        raise ValueError("worker daemon failed to start; inspect daemon.log")
    finally:
        try:
            lock.rmdir()
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--stdio", action="store_true")
    parser.add_argument("--daemon", action="store_true")
    parser.add_argument("--run-job")
    parser.add_argument("--trusted-local", action="store_true")
    args = parser.parse_args()
    root = Path(args.state).resolve()
    root.mkdir(parents=True, exist_ok=True)
    root.chmod(0o700)
    if args.run_job:
        supervise_job(root, job_dir(root, args.run_job))
    elif args.daemon:
        serve(root, args.trusted_local)
    elif args.stdio:
        endpoint = ensure_daemon(root, args.trusted_local)
        while True:
            raw = sys.stdin.buffer.readline(MAX_RPC + 1)
            if not raw:
                break
            request = {}
            try:
                if len(raw) > MAX_RPC:
                    raise ValueError("request exceeds size limit")
                request = json.loads(raw)
                response = relay_call(endpoint, request)
            except Exception as error:
                response = {"jsonrpc": "2.0", "id": request.get("id"), "error": {"code": -32000, "message": str(error)[:4096]}}
            sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            sys.stdout.flush()
    else:
        parser.error("select --stdio, --daemon or --run-job")


if __name__ == "__main__":
    main()
