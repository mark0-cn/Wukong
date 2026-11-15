"""Jobs."""

from pathlib import Path
from collections.abc import Callable
from typing import Any
import hashlib
import json
import subprocess
import sys
import threading
import time
import uuid
from wukong_worker import BRIDGE_PATH, PACKAGE_ROOT
from wukong_worker.artifacts.common import MAX_FILE, artifact, confined, job_dir, public_file, tail
from wukong_worker.resources import TERMINAL
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_analysis.validation import bounded_int, parse_address
from wukong_worker.host.environment import safe_env
from wukong_worker.execution.planning import JobPlan, prepare_job
from wukong_worker.execution.processes import process_identity
from wukong_worker.resources.processes import ProcessResources


class JobManager:
    def __init__(
        self,
        root: Path,
        trusted: bool,
        isolated: bool,
        observe: Callable[[], dict[str, Any]] | None = None,
    ):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.observe = observe
        self.process_resources = ProcessResources()
        self.lock = threading.RLock()
        (root / "jobs").mkdir(exist_ok=True)

    def start_job(self, params: dict[str, Any], kind: str = "process") -> dict[str, Any]:
        plan = prepare_job(self.root, params, kind, trusted=self.trusted, isolated=self.isolated)
        with self.lock:
            return self._reserve_and_start(plan)

    def _reserve_and_start(self, plan: JobPlan) -> dict[str, Any]:
        # Caller holds the admission lock through duplicate lookup, durable
        # reservation and process creation. Failed/uncertain jobs are never respawned.
        spec = plan.spec
        fingerprint = plan.fingerprint
        call_id = spec.get("call_id")
        kind = spec["kind"]
        if call_id:
            for path in (self.root / "jobs").glob("*/status.json"):
                existing = load_json(path)
                if existing.get("call_id") == call_id:
                    if existing["fingerprint"] != fingerprint:
                        raise ValueError("call_id already used with a different request")
                    return self.status(existing["job_id"])
        identifier = (
            hashlib.sha256((kind + ":" + str(spec["request_id"])).encode()).hexdigest()[:32]
            if spec.get("request_id")
            else uuid.uuid4().hex
        )
        directory = self.root / "jobs" / identifier
        if directory.exists():
            existing = load_json(directory / "status.json")
            if existing.get("fingerprint") != fingerprint:
                raise ValueError("request_id already used with different job parameters")
            return self.status(identifier)
        directory.mkdir()
        if self.isolated:
            import wukong_worker.platform.isolation as isolation

            spec["execution_uid"] = isolation.allocate_uid(self.root)
        meta = {
            "job_id": identifier,
            "kind": kind,
            "state": "queued",
            "created_at": time.time(),
            "call_id": call_id,
            "fingerprint": fingerprint,
            "runner_token": spec["runner_token"],
            "artifacts_dir": "jobs/" + identifier,
        }
        atomic_json(directory / "spec.json", spec)
        atomic_json(directory / "status.json", meta)
        with (directory / "supervisor.log").open("ab") as log:
            process = self.process_resources.spawn(
                [
                    sys.executable,
                    str(BRIDGE_PATH),
                    "--state",
                    str(self.root),
                    "--run-job",
                    identifier,
                ],
                env=safe_env(),
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                durable=True,
            )
        # The registry waits the child without cancelling its durable work.
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
                if current_identity == expected_identity and current_identity not in {
                    "missing",
                    "unknown",
                }:
                    meta["reconciliation"] = "supervisor_alive_heartbeat_stale"
                    live = True
                elif "supervisor_pid" not in meta:
                    # start_job wrote queued before spawning the supervisor; a
                    # missing pid means the spawn raised and this job can never
                    # start. It is lost, not merely unconfirmed.
                    meta["reconciliation"] = "supervisor_never_created"
                    live = False
                elif current_identity == "unknown" or (
                    expected_identity == "unknown" and current_identity != "missing"
                ):
                    meta["reconciliation"] = "supervisor_liveness_unconfirmed"
                    live = True
            if not live:
                # Recheck under the shared lock; never restart an ambiguous job.
                with self.lock:
                    fresh = load_json(directory / "status.json")
                    if fresh["state"] not in TERMINAL:
                        reason = (
                            "supervisor process was never created; effects unknown"
                            if "supervisor_pid" not in fresh
                            else "supervisor process creation identity is absent or changed; effects unknown"
                        )
                        fresh.update({"state": "lost", "error": reason, "finished_at": time.time()})
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
                import wukong_worker.platform.isolation as isolation

                try:
                    experiment = load_json(directory / "experiment.json")
                    stats, paths = isolation.collect(
                        self.root, experiment, directory / "fuzz-artifacts", "fuzz", safe_env()
                    )
                    isolated_crashes = [artifact(self.root, path) for path in paths]
                except Exception as error:
                    meta["artifact_collection_error"] = str(error)[:1024]
            for path in sorted(out.glob("*/fuzzer_stats"))[:32]:
                stats[path.parent.name] = dict(
                    line.split(":", 1)[0].strip()
                    and [line.split(":", 1)[0].strip(), line.split(":", 1)[1].strip()]
                    for line in path.read_text(errors="replace").splitlines()
                    if ":" in line
                )
            meta["fuzzer_stats"] = stats
            meta["crashes"] = (
                isolated_crashes
                if isolated_crashes is not None
                else [
                    artifact(self.root, path)
                    for path in sorted(out.glob("*/crashes/id*"))[:128]
                    if path.is_file()
                ]
            )
            meta["crash_reporting_override"] = (
                spec.get("env", {}).get("AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES") == "1"
            )
            meta["crash_collection_disabled"] = spec.get("env", {}).get("AFL_SKIP_CRASHES") == "1"
        return meta

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

    def query(self, params):
        return self.status(params["job_id"])

    def list(self, params):
        limit = bounded_int(params.get("limit"), 64, 1, 64)
        after = params.get("after") or ""
        paths = sorted((self.root / "jobs").glob("*/status.json"))
        page = [path for path in paths if path.parent.name > after][:limit]
        return [self.status(path.parent.name) for path in page]

    def cancel(self, params):
        directory = job_dir(self.root, params["job_id"])
        (directory / "cancel").touch()
        return self.status(params["job_id"])

    def artifacts(self, params):
        directory = job_dir(self.root, params["job_id"])
        files = []
        for path in sorted(directory.rglob("*")):
            if (
                path.is_file()
                and not path.is_symlink()
                and path.name
                not in {"spec.json", "status.json", "heartbeat.json", "cancel", "supervisor.log"}
            ):
                files.append(artifact(self.root, path))
                if len(files) == 512:
                    break
        return files

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
