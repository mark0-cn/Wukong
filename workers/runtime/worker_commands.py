"""Commands."""
from pathlib import Path
import os
import shutil
import threading
import uuid
from worker_common import bounded_int, public_file, safe_env
from worker_processes import run_process

class CommandService:

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.lock = threading.RLock()

    def run(self, params):
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
        if os.name == "nt":
            # Trusted-local Windows host: the system temp directory, since
            # "/tmp" does not exist and the state root may be read-only.
            base = Path(os.environ.get("TEMP") or os.environ.get("TMP") or str(self.root))
        else:
            base = Path("/tmp")
        workdir = base / "wukong-exec" / uuid.uuid4().hex
        workdir.mkdir(parents=True)
        env = safe_env(params.get("env"))
        state_value, job_value = str(self.root), str(workdir)
        if os.name == "nt":
            # MSYS bash handles drive-letter paths in forward-slash form.
            state_value = state_value.replace("\\", "/")
            job_value = job_value.replace("\\", "/")
        env.update({"WUKONG_STATE_DIR": state_value, "WUKONG_JOB_DIR": job_value})
        experiment = None
        cwd = workdir
        resolved_target = None
        if params.get("target") is not None:
            target = params["target"]
            if not isinstance(target, str):
                raise ValueError("target must be a staged worker path")
            resolved_target = public_file(self.root, target)
            if not resolved_target.is_file():
                raise ValueError("exec target must be a staged file")
        if self.isolated:
            import isolation
            experiment = isolation.sandbox(self.root, isolation.allocate_uid(self.root))
            if resolved_target is not None:
                env["TARGET"] = str(isolation.stage_file(resolved_target, experiment, "target.bin", True))
            isolation.seal(experiment)
            cwd = Path(experiment["work"])
        elif resolved_target is not None:
            env["TARGET"] = str(resolved_target)
        if os.name != "nt":
            shell = ["bash", "-lc"]
        elif git := shutil.which("git"):
            # Prefer the bash that ships with Git for Windows; the WSL
            # launcher at system32/bash.exe may have no usable distro.
            candidate = Path(git).resolve().parent.parent / "bin" / "bash.exe"
            if candidate.is_file():
                shell = [str(candidate), "-lc"]
            else:
                shell = ["cmd", "/C"]
        elif shutil.which("bash"):
            shell = ["bash", "-lc"]
        else:
            shell = ["cmd", "/C"]
        result = run_process([*shell, command], cwd, env, timeout_secs, output_limit,
                             workdir / "output", None, experiment)
        result["command"] = command
        return result
