"""Interactive."""
from pathlib import Path
import os
import subprocess
import threading
import time
import uuid
from worker_common import atomic_json, safe_env
from worker_processes import attach_windows_job, detached_options, terminate_tree

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
        self.closed = False
        self.buffer = bytearray()
        self.total_bytes = 0
        self.last_data = 0.0
        self.experiment = None
        argv = list(params["argv"])
        if not argv or not all(isinstance(argument, str) for argument in argv):
            raise ValueError("interactive argv must be a nonempty list of strings")
        self.isolated = isolated
        launch=None
        if params.get("launch_spec"):
            from worker_launch import resolve
            from worker_common import confined
            launch=resolve(root,params,isolated,confined(root,params["input"],True) if params.get("input") else None)
            argv,self.experiment=launch["argv"],launch["experiment"]
        elif isolated:
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
        env = safe_env(params.get("env"))
        if launch: cwd,env=launch["cwd"],launch["env"]
        credential_options = {}
        if self.experiment:
            import isolation
            credential_options = isolation.credentials(self.experiment)
            env.update({"HOME": self.experiment["work"], "TMPDIR": self.experiment["work"]})
        self.pty_master=None
        terminal_slave=None
        if launch and launch["input_mode"]=="pty":
            import pty
            self.pty_master,terminal_slave=pty.openpty()
        self.process = subprocess.Popen(argv, cwd=cwd, env=env,
                                        stdin=terminal_slave if terminal_slave is not None else subprocess.PIPE, stdout=terminal_slave if terminal_slave is not None else subprocess.PIPE,
                                        stderr=subprocess.STDOUT, bufsize=0, **detached_options(), **credential_options)
        if terminal_slave is not None:
            os.close(terminal_slave)
            self.process.stdin=os.fdopen(os.dup(self.pty_master),"wb",buffering=0)
            self.process.stdout=os.fdopen(self.pty_master,"rb",buffering=0)
        attach_windows_job(self.process)
        threading.Thread(target=self.drain, daemon=True).start()
        self.persist("running")

    def persist(self, state):
        atomic_json(self.directory / "session.json", {
            "session_id": self.id, "state": state, "pid": self.process.pid,
            "daemon_pid": os.getpid(), "updated_at": time.time()})

    def drain(self):
        while True:
            try: chunk = self.process.stdout.read(4096)
            except OSError as error:
                if self.pty_master is not None and error.errno==5: break
                raise
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
            state = "closed" if self.closed else "exited"
        self.persist(state)

    def write(self, data, read_timeout_secs, quiet_ms=250):
        with self.lock:
            exited = self.dead or self.process.poll() is not None
            if not exited:
                self.process.stdin.write(data)
                self.process.stdin.flush()
                self.last_data = time.time()
        if exited:
            return self.snapshot("exited", data_sent=False)
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
            self.closed = True
            terminate_tree(self.process)
        self.persist("closed")
        return {"session_id": self.id, "state": "closed"}
