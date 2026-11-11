"""Persistent DbgEng adapter. Production target creation belongs to the Windows broker."""
import collections
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import threading
import time
import uuid

from worker_common import WorkerError, atomic_json, load_json, public_file, safe_env, sha256_file
from worker_debug_operations import inspect_request, control_request
from worker_processes import attach_windows_job, detached_options, terminate_tree

MAX_FRAME = 1024 * 1024
MAX_TRANSCRIPT = 8 * 1024 * 1024


def profile(root):
    configuration = load_json(Path(root) / "windows.backend.json")
    helper = Path(configuration["helper"]).resolve(strict=True)
    directory = Path(configuration["engine_dir"]).resolve(strict=True)
    engine = directory / "dbgeng.dll"
    if not helper.is_file() or sha256_file(helper) != configuration["helper_sha256"]:
        raise ValueError("DbgEng helper content identity does not match its installed profile")
    if not engine.is_file() or sha256_file(engine) != configuration["engine_sha256"]:
        raise ValueError("DbgEng library content identity does not match its installed profile")
    symbols = Path(root) / "files" / "symbols"
    symbols.mkdir(parents=True, exist_ok=True)
    return helper, directory, symbols, configuration


def architecture(path):
    with Path(path).open("rb") as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            return None
        offset = struct.unpack_from("<I", header, 60)[0]
        if offset > 1024 * 1024:
            raise ValueError("PE header offset exceeds inspection limit")
        stream.seek(offset)
        signature = stream.read(6)
        if len(signature) != 6 or signature[:4] != b"PE\0\0":
            raise ValueError("invalid PE header")
        machine = struct.unpack_from("<H", signature, 4)[0]
        if machine not in {0x14c, 0x8664}:
            raise ValueError("DbgEng worker supports x86 and x64 targets")
        return "x86" if machine == 0x14c else "x64"


def doctor(root, trusted=False):
    if os.name != "nt" or not (Path(root) / "windows.backend.json").is_file():
        return {"available":False,"reason":"no installed Windows backend profile"}
    try:
        helper, directory, symbols, configuration = profile(root)
        probe = subprocess.run([str(helper),str(directory),str(symbols)],
            input=b'{"id":1,"operation":"doctor"}\n{"id":2,"operation":"close"}\n',
            env=safe_env(), capture_output=True, timeout=10, **detached_options())
        if probe.returncode != 0:
            diagnostic = probe.stderr[-384:].decode("utf-8", errors="replace")
            raise ValueError(f"native helper probe failed (exit {probe.returncode}): {diagnostic}")
        if len(probe.stdout) > MAX_FRAME:
            raise ValueError("native helper probe output exceeds its bound")
        response = json.loads(probe.stdout.splitlines()[0])
        if "error" in response or response.get("result",{}).get("engine") != "dbgeng":
            detail = str(response.get("error", "unexpected engine identity"))[-384:]
            raise ValueError(f"native helper returned an invalid engine probe: {detail}")
        return {"available":True,"trusted_fixture_only":trusted,
                "production_ready":False,"engine_sha256":configuration["engine_sha256"],
                "helper_sha256":configuration["helper_sha256"],"probe":response["result"]}
    except (OSError, ValueError, KeyError, IndexError, subprocess.TimeoutExpired) as error:
        return {"available":False,"reason":str(error)[:512]}


class DebugSession:
    engine = "dbgeng"

    def __init__(self, root, params, trusted=False, broker=None):
        if os.name != "nt":
            raise ValueError("DbgEng requires a Windows worker")
        if not trusted and broker is None:
            raise ValueError("production Windows debugging requires the managed isolation broker")
        self.id = uuid.uuid4().hex
        self.root = Path(root).resolve()
        self.directory = self.root / "debug" / self.id
        self.directory.mkdir(parents=True)
        self.lock = threading.RLock()
        self.records = queue.Queue(maxsize=32)
        self.events = collections.deque(maxlen=256)
        self.dead = False
        self.token = 0
        self.transcript_bytes = 0
        self.transcript_truncated = False
        self.inferior = None
        self.mode = params.get("mode", "launch")
        self.process = None
        target = public_file(self.root, params["target"])
        self.target_sha256 = sha256_file(target)
        self.dump_sha256 = None
        if self.mode not in {"launch", "dump"}:
            raise ValueError("invalid Windows debugger mode")
        helper, directory, symbols, self.configuration = profile(root)
        try:
            self.process = subprocess.Popen([str(helper), str(directory), str(symbols)],
                cwd=self.directory, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env=safe_env(), **detached_options())
            attach_windows_job(self.process)
            threading.Thread(target=self._read, daemon=True).start()
            threading.Thread(target=self._drain_stderr, daemon=True).start()
            timeout = params.get("timeout_secs", 30)
            if self.mode == "dump":
                if params.get("args") or params.get("launch_spec") or params.get("input"):
                    raise ValueError("dump inspection cannot supply launch parameters")
                dump = public_file(self.root, params["dump"])
                self.dump_sha256 = sha256_file(dump)
                request = {"operation":"open", "mode":"dump", "dump":str(dump), "timeout_secs":timeout}
                detected = architecture(target)
                if detected:
                    request["architecture"] = detected
            else:
                detected = architecture(target)
                if detected is None:
                    raise ValueError("Windows launch requires a staged PE executable")
                if broker is not None:
                    self.inferior = broker.start_debuggee(self.directory, target, params)
                else:
                    # Only explicitly trusted development fixtures reach this path.
                    self.inferior = subprocess.Popen([str(target), *params.get("args", [])], cwd=self.directory,
                        env=safe_env(), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                        **detached_options())
                    attach_windows_job(self.inferior)
                request = {"operation":"open", "mode":"attach", "pid":self.inferior.pid,
                           "architecture":detected, "timeout_secs":timeout}
            opened = self._request(request, timeout + 5)
            self.events.append({"engine":"dbgeng", "message":"opened", "payload":opened})
            self.persist("running")
        except Exception:
            self.dead = True
            if self.process is not None:
                terminate_tree(self.process)
            if self.inferior is not None:
                terminate_tree(self.inferior)
            self.persist("lost")
            raise

    def persist(self, state):
        atomic_json(self.directory / "session.json", {"session_id":self.id, "engine":"dbgeng", "mode":self.mode,
            "state":state, "daemon_pid":os.getpid(), "helper_pid":self.process.pid if self.process else None,
            "inferior_pid":self.inferior.pid if self.inferior else None, "target_sha256":self.target_sha256,
            "dump_sha256":self.dump_sha256, "transcript_truncated":self.transcript_truncated, "updated_at":time.time()})

    def _read(self):
        path = self.directory / "transcript.jsonl"
        with path.open("ab") as transcript:
            for line in iter(lambda:self.process.stdout.readline(MAX_FRAME + 1), b""):
                if len(line) > MAX_FRAME or not line.endswith(b"\n"):
                    self.dead = True
                    terminate_tree(self.process)
                    break
                if self.transcript_bytes + len(line) <= MAX_TRANSCRIPT:
                    transcript.write(line)
                    transcript.flush()
                    self.transcript_bytes += len(line)
                else:
                    self.transcript_truncated = True
                try:
                    response = json.loads(line)
                    if not isinstance(response, dict):
                        raise ValueError("helper reply is not an object")
                    self.records.put_nowait(response)
                except (ValueError, queue.Full):
                    self.dead = True
                    terminate_tree(self.process)
                    break
        self.dead = True

    def _drain_stderr(self):
        # Drain continuously while retaining a bounded diagnostic artifact.
        retained = 0
        with (self.directory / "helper-stderr.txt").open("wb") as stream:
            for chunk in iter(lambda:self.process.stderr.read(4096), b""):
                available = min(len(chunk), 65536 - retained)
                if available > 0:
                    stream.write(chunk[:available])
                    retained += available

    def _request(self, request, timeout):
        with self.lock:
            if self.dead or self.process.poll() is not None:
                raise WorkerError("session_lost", "DbgEng helper is no longer available")
            self.token += 1
            request = {**request, "id":self.token}
            encoded = json.dumps(request, ensure_ascii=False).encode() + b"\n"
            if len(encoded) > MAX_FRAME:
                raise ValueError("DbgEng request exceeds frame limit")
            self.process.stdin.write(encoded)
            self.process.stdin.flush()
            deadline = time.monotonic() + timeout
            while True:
                try:
                    response = self.records.get(timeout=max(0.01, min(0.1, deadline - time.monotonic())))
                    break
                except queue.Empty:
                    if self.dead or self.process.poll() is not None:
                        self.dead = True
                        if self.inferior is not None:
                            terminate_tree(self.inferior)
                        self.persist("lost")
                        raise WorkerError("session_lost", "DbgEng helper exited; inspect its stderr artifact") from None
                    if time.monotonic() >= deadline:
                        self.dead = True
                        terminate_tree(self.process)
                        if self.inferior is not None:
                            terminate_tree(self.inferior)
                        self.persist("lost")
                        raise WorkerError("outcome_unknown", "DbgEng response timeout; session invalidated") from None
            if response.get("id") != self.token:
                self.dead = True
                terminate_tree(self.process)
                raise WorkerError("outcome_unknown", "DbgEng reply identity mismatch")
            if "error" in response:
                lost = bool(response.get("session_invalidated"))
                if lost:
                    self.dead = True
                    self.persist("lost")
                raise WorkerError("outcome_unknown" if lost else "invalid_request", str(response["error"])[:4096])
            return response["result"]

    def _envelope(self, result):
        return {**result, "session_id":self.id, "engine":"dbgeng", "target_sha256":self.target_sha256,
                "dump_sha256":self.dump_sha256, "transcript":"debug/" + self.id + "/transcript.jsonl",
                "transcript_truncated":self.transcript_truncated,
                "engine_sha256":self.configuration["engine_sha256"], "helper_sha256":self.configuration["helper_sha256"]}

    def inspect(self, params, timeout):
        request = inspect_request(params)
        return self._envelope(self._request({**request, "operation":"inspect"}, timeout))

    def control(self, params, timeout):
        request = control_request(params)
        if self.mode == "dump":
            raise ValueError("dump sessions do not support execution control")
        result = self._request({**request, "operation":"control", "timeout_secs":timeout}, timeout + 5)
        if result.get("event"):
            self.events.append({"engine":"dbgeng", "message":"stopped", "payload":result["event"]})
        return self._envelope(result)

    def command(self, command, timeout, internal=False):
        raise ValueError("raw WinDbg commands are unavailable; use structured debugger inspection and control")

    def profile(self):
        modules = self.inspect({"kind":"modules", "limit":256}, 30)
        return {**modules, "process":{"pid":self.inferior.pid if self.inferior else None,
                "modules":modules["data"]["modules"], "runtime_identity_confirmed":False}}

    def snapshot(self):
        return self._envelope({"registers":self.inspect({"kind":"registers"},30)["data"],
            "frames":self.inspect({"kind":"stack"},30)["data"],
            "exception":self.inspect({"kind":"exception"},30)["data"],
            "profile":self.profile()["process"], "signature_strength":"unresolved"})

    def close(self):
        with self.lock:
            if not self.dead:
                try:
                    self._request({"operation":"close"}, 5)
                except (OSError, ValueError, WorkerError):
                    pass
            self.dead = True
            terminate_tree(self.process)
            if self.inferior is not None:
                terminate_tree(self.inferior)
            self.persist("closed")
            return self._envelope({"state":"closed"})
