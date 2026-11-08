"""Commands."""
from pathlib import Path
import os
import threading
import uuid
from worker_common import bounded_int, safe_env
from worker_processes import run_process

class CommandService:
    methods = frozenset(['exec.run'])

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.lock = threading.RLock()

    def dispatch(self, method, params):
        if method == "exec.run":
            command = params.get("command")
            if not isinstance(command, str) or not command.strip() or len(command) > 65536 or "\0" in command:
                raise ValueError("command must be a nonempty string <=64KiB without NUL bytes")
            if not self.trusted and not self.isolated:
                raise ValueError("production shell execution requires a root Linux worker with isolated processes")
            timeout_secs = bounded_int(params.get("timeout_secs"), 30, 1, 120)
            output_limit = bounded_int(params.get("output_limit"), 1048576, 1024, 8 * 1048576)
            # Synchronous, Strix-style shell execution: run in a fresh
            # directory under the exec tmpfs (the state volume is data, not
            # code), bounded output, hard timeout, optional uid isolation.
            workdir = Path("/tmp") / "re-agent-exec" / uuid.uuid4().hex
            workdir.mkdir(parents=True)
            env = safe_env(params.get("env"))
            env.update({"RE_AGENT_STATE_DIR": str(self.root), "RE_AGENT_JOB_DIR": str(workdir)})
            experiment = None
            cwd = workdir
            if self.isolated:
                import isolation
                experiment = isolation.sandbox(self.root, isolation.allocate_uid(self.root))
                isolation.seal(experiment)
                cwd = Path(experiment["work"])
            shell = ["bash", "-lc"] if os.name != "nt" else ["cmd", "/C"]
            result = run_process([*shell, command], cwd, env, timeout_secs, output_limit,
                                 workdir / "output", None, experiment)
            result["command"] = command
            return result
        raise ValueError("unsupported CommandService method: " + method)
