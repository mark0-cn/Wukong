"""Tool-specific job admission; the manager owns only resource lifecycle."""

from pathlib import Path
from collections.abc import Callable
from typing import Any
import hashlib
import json
import uuid
from wukong_worker import PACKAGE_ROOT
from wukong_worker.artifacts.common import confined, public_file
from wukong_analysis.validation import bounded_int, parse_address
from wukong_worker.operations.planning import prepare_job
from wukong_worker.resources.jobs import JobManager
from wukong_worker.operations.fuzz_observation import observe


class JobService:
    def __init__(
        self,
        root: Path,
        trusted: bool,
        isolated: bool,
        jobs: JobManager,
        observe: Callable[[], dict[str, Any]] | None = None,
    ):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.jobs, self.observe = jobs, observe

    def start_job(self, params: dict[str, Any], kind: str = "process") -> dict[str, Any]:
        plan = prepare_job(self.root, params, kind, trusted=self.trusted, isolated=self.isolated)
        return observe(self.root, self.jobs.start(plan))

    def doctor(self):
        return self.observe() if self.observe else {"capabilities": {}}

    def solve(self, params):
        target = public_file(self.root, params["target"])
        if not target.is_file():
            raise ValueError("angr target must be a staged file")
        try:
            find = [parse_address(item) for item in params.get("find")]
            avoid = [parse_address(item) for item in (params.get("avoid") or [])]
        except TypeError as error:
            raise ValueError(
                "find/avoid must be arrays of module-relative addresses: %s" % error
            ) from error
        if not find:
            raise ValueError("find must be a nonempty list of module-relative addresses")
        stdin_length = bounded_int(params.get("stdin_length"), 64, 1, 4096)
        max_steps = bounded_int(params.get("max_steps"), 50000, 1, 10_000_000)
        payload = json.dumps(
            {
                "find": find,
                "avoid": avoid,
                "stdin_length": stdin_length,
                "max_steps": max_steps,
                "printable_only": bool(params.get("printable_only", True)),
            }
        )
        script = str(PACKAGE_ROOT / "execution/angr_solve.py")
        # The target travels as an argv entry so isolated jobs stage it
        # into the sandbox like every other worker file reference.
        spec = {
            "argv": ["python3", script, payload, str(params["target"])],
            "call_id": params.get("call_id"),
            "request_id": params.get("request_id"),
            "timeout_secs": bounded_int(params.get("timeout_secs"), 600, 10, 3600),
            "output_limit": 1024 * 1024,
        }
        return self.start_job(spec)

    def python(self, params):
        source = params.get("source")
        if (
            not isinstance(source, str)
            or not source.strip()
            or len(source) > 262144
            or "\0" in source
        ):
            raise ValueError("source must be a nonempty string <=256KiB without NUL bytes")
        import py_compile

        script_rel = "files/py_run_" + uuid.uuid4().hex + ".py"
        script = confined(self.root, script_rel)
        script.parent.mkdir(parents=True, exist_ok=True)
        script.write_text(source, encoding="utf-8")
        script.chmod(0o444)
        # Syntax errors fail HERE, inline, instead of costing a job round-trip.
        try:
            py_compile.compile(str(script), doraise=True)
        except py_compile.PyCompileError as error:
            raise ValueError("syntax error in source: %s" % str(error)[:800]) from error
        argv = ["python3", script_rel]
        argv.extend(str(argument) for argument in (params.get("argv") or [])[:64])
        if params.get("target") is not None:
            # The target travels as an argv entry so isolated jobs stage
            # it into the sandbox like every other worker file reference.
            argv.append(str(params["target"]))
        spec = {
            "argv": argv,
            "executable_inputs": [params["target"]] if params.get("target") else [],
            "call_id": params.get("call_id"),
            "request_id": params.get("request_id"),
            "timeout_secs": bounded_int(params.get("timeout_secs"), 120, 10, 3600),
            "output_limit": bounded_int(params.get("output_limit"), 1048576, 1024, 8 * 1048576),
        }
        return self.start_job(spec)

    def start(self, params):
        return self.start_job(params)

    def fuzz(self, params):
        doctor = self.doctor()
        if not doctor["capabilities"]["fuzz_qemu"]:
            raise ValueError(
                "AFL++ QEMU capability unavailable; inspect doctor before selecting backend"
            )
        target = public_file(self.root, params["target"])
        seeds = public_file(self.root, params["seeds"])
        if not seeds.is_dir() or not any(p.is_file() for p in seeds.iterdir()):
            raise ValueError("nonempty seed directory required")
        key = params.get("request_id") or params.get("call_id")
        identifier = (
            hashlib.sha256(("fuzz:" + str(key)).encode()).hexdigest()[:32]
            if key
            else uuid.uuid4().hex
        )
        output_relative = "fuzz/" + identifier
        output = confined(self.root, output_relative)
        output.mkdir(parents=True, exist_ok=True)
        seconds = bounded_int(params.get("timeout_secs"), 60, 1, 86400)
        timeout_ms = bounded_int(params.get("exec_timeout_ms"), 1000, 10, 60000)
        memory_mb = bounded_int(params.get("memory_mb"), 256, 16, 65536)
        argv = [
            "afl-fuzz",
            "-Q",
            "-i",
            str(seeds),
            "-o",
            str(output),
            "-V",
            str(seconds),
            "-t",
            str(timeout_ms),
            "-m",
            str(memory_mb),
            "--",
            str(target),
        ] + params.get("args", ["@@"])
        # Do not inject AFL_SKIP_CRASHES: crashing inputs are the campaign's
        # product and must be saved under crashes/. Callers may still pass
        # it (or any AFL_* override) explicitly via env, and fuzz.status
        # reports that crash collection is disabled.
        spec = {
            **params,
            "argv": argv,
            "afl_output": output_relative,
            "timeout_secs": seconds + 15,
            "env": {**params.get("env", {}), "AFL_SKIP_CPUFREQ": "1", "AFL_NO_UI": "1"},
        }
        return self.start_job(spec, "fuzz")

    def query(self, params):
        return observe(self.root, self.jobs.query(params))

    def list(self, params):
        return [observe(self.root, job) for job in self.jobs.list(params)]

    def cancel(self, params):
        return observe(self.root, self.jobs.cancel(params))
