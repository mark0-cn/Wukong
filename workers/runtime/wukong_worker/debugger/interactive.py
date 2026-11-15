"""Interactive."""

from pathlib import Path
import os
import queue
import subprocess
import threading
import time
import uuid
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.host.environment import safe_env
from wukong_worker.resources.processes import ProcessResources


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
        self.operation_lock = threading.Lock()
        self.close_lock = threading.Lock()
        self.drained = threading.Event()
        self.dead = False
        self.closed = False
        self.buffer = bytearray()
        self.total_bytes = 0
        self.last_data = 0.0
        self.experiment = None
        argv = list(params["argv"])
        if not argv or not all(isinstance(argument, str) for argument in argv):
            raise ValueError("interactive argv must be a nonempty list of strings")
        self.isolated = isolated
        launch = None
        if params.get("launch_spec"):
            from wukong_worker.execution.launch import resolve
            from wukong_worker.artifacts.common import public_file

            launch = resolve(
                root,
                params,
                isolated,
                public_file(root, params["input"]) if params.get("input") else None,
            )
            argv, self.experiment = launch["argv"], launch["experiment"]
        elif isolated:
            import wukong_worker.platform.isolation as isolation

            self.experiment = isolation.sandbox(root, isolation.allocate_uid(root))
            for index, argument in enumerate(argv):
                path = Path(argument)
                absolute = path.resolve() if path.is_absolute() else (root / path).resolve()
                relative_input = not path.is_absolute() and argument.replace("\\", "/").split(
                    "/", 1
                )[0] in {"files", "jobs", "fuzz"}
                if (path.is_absolute() and root in absolute.parents) or relative_input:
                    if absolute.is_file():
                        argv[index] = str(
                            isolation.stage_file(absolute, self.experiment, executable=(index == 0))
                        )
                    elif absolute.is_dir():
                        argv[index] = str(
                            isolation.stage_directory(
                                absolute, self.experiment, "directory-" + str(index)
                            )
                        )
                    else:
                        raise ValueError(
                            "isolated interactive argv paths must exist under the state directory"
                        )
            isolation.seal(self.experiment)
        cwd = Path(self.experiment["work"]) if self.experiment else self.directory
        env = safe_env(params.get("env"))
        if launch:
            cwd, env = launch["cwd"], launch["env"]
        credential_options = {}
        if self.experiment:
            import wukong_worker.platform.isolation as isolation

            credential_options = isolation.credentials(self.experiment)
            env.update({"HOME": self.experiment["work"], "TMPDIR": self.experiment["work"]})
        self.pty_master = None
        terminal_slave = None
        if launch and launch["input_mode"] == "pty":
            import pty

            self.pty_master, terminal_slave = pty.openpty()
        self.process_resources = ProcessResources()
        owned_master = self.pty_master
        try:
            self.process = self.process_resources.spawn(
                argv,
                cwd=cwd,
                env=env,
                stdin=terminal_slave if terminal_slave is not None else subprocess.PIPE,
                stdout=terminal_slave if terminal_slave is not None else subprocess.PIPE,
                stderr=subprocess.STDOUT,
                bufsize=0,
                **credential_options,
            )
            if terminal_slave is not None:
                self.process.stdin = os.fdopen(os.dup(self.pty_master), "wb", buffering=0)
                self.process.stdout = os.fdopen(self.pty_master, "rb", buffering=0)
                owned_master = None  # stdout now owns the descriptor
            self.persist("running")
            threading.Thread(target=self.drain, daemon=True).start()
        except BaseException:
            self.dead = True
            self.process_resources.close()
            if owned_master is not None:
                os.close(owned_master)
            if hasattr(self, "process"):
                for stream in (self.process.stdin, self.process.stdout):
                    if stream is not None:
                        stream.close()
            raise
        finally:
            if terminal_slave is not None:
                os.close(terminal_slave)

    def persist(self, state):
        atomic_json(
            self.directory / "session.json",
            {
                "session_id": self.id,
                "state": state,
                "pid": self.process.pid,
                "daemon_pid": os.getpid(),
                "updated_at": time.time(),
            },
        )

    def drain(self):
        while True:
            try:
                chunk = self.process.stdout.read(4096)
            except OSError as error:
                if self.pty_master is not None and error.errno == 5:
                    break
                raise
            if not chunk:
                break
            with self.lock:
                self.buffer.extend(chunk)
                self.total_bytes += len(chunk)
                if len(self.buffer) > 1024 * 1024:
                    del self.buffer[: len(self.buffer) - 1024 * 1024]
                self.last_data = time.time()
        with self.lock:
            self.dead = True
            state = "closed" if self.closed else "exited"
            self.drained.set()
        self.persist(state)

    def write(self, data, read_timeout_secs, quiet_ms=250):
        with self.operation_lock:
            return self.exchange(data, read_timeout_secs, quiet_ms)

    def exchange(self, data, read_timeout_secs, quiet_ms):
        deadline = time.monotonic() + read_timeout_secs
        with self.lock:
            exited = self.dead or self.process.poll() is not None
        if exited:
            return self.exited_snapshot(deadline, data_sent=False)
        self.send_input(data, deadline)
        with self.lock:
            self.last_data = time.time()
        while time.monotonic() < deadline:
            with self.lock:
                exited = self.process.poll() is not None
                quiet = bool(self.buffer) and time.time() - self.last_data >= quiet_ms / 1000.0
            if exited:
                return self.exited_snapshot(deadline)
            if quiet:
                return self.snapshot("running")
            time.sleep(0.02)
        return self.snapshot("running", timed_out=True)

    def send_input(self, data: bytes, deadline: float) -> None:
        """Own one bounded write; partial/unknown delivery must never be retried."""
        if not data:
            return
        completed = queue.Queue(maxsize=1)

        def send():
            try:
                remaining = memoryview(data)
                while remaining:
                    written = self.process.stdin.write(remaining)
                    if not written:
                        raise BrokenPipeError("interactive stdin closed during write")
                    remaining = remaining[written:]
                self.process.stdin.flush()
                completed.put(None)
            except (OSError, ValueError) as error:
                completed.put(error)

        sender = threading.Thread(target=send, daemon=True)
        sender.start()
        try:
            error = completed.get(timeout=max(0, deadline - time.monotonic()))
        except queue.Empty as error:
            # Terminate through the session owner, without holding the output
            # lock. Killing the process tree releases a blocked pipe writer.
            self.close()
            sender.join(timeout=5)
            raise ValueError(
                "interactive stdin timeout; session closed, delivery uncertain"
            ) from error
        sender.join(timeout=5)
        if error is not None:
            self.close()
            raise ValueError(
                "interactive stdin failed; session closed, delivery uncertain"
            ) from error

    def exited_snapshot(self, deadline, data_sent=True):
        # A signaled leader can still have unread bytes in its stdout pipe.
        # Keep the caller's read deadline, including inherited open writers.
        complete = self.drained.wait(max(0, deadline - time.monotonic()))
        return self.snapshot("exited", data_sent=data_sent, timed_out=not complete)

    def snapshot(self, state, data_sent=True, timed_out=False):
        with self.lock:
            captured = bytes(self.buffer)
            self.buffer.clear()
        text = captured.decode("utf-8", errors="replace")
        return {
            "session_id": self.id,
            "state": state,
            "captured": text,
            "captured_bytes": len(captured),
            "truncated": len(text) > 65536,
            "total_bytes": self.total_bytes,
            "exit_code": self.process.poll(),
            "timed_out": timed_out,
            "data_sent": data_sent,
        }

    def close(self):
        with self.close_lock:
            with self.lock:
                self.dead = True
                self.closed = True
            self.process_resources.close()
            self.persist("closed")
        return {"session_id": self.id, "state": "closed"}
