"""Jobs."""

from pathlib import Path
from typing import Any
import hashlib
import subprocess
import sys
import time
import uuid
from wukong_worker import BRIDGE_PATH
from wukong_worker.artifacts.common import artifact, job_dir, tail
from wukong_worker.artifacts.metadata import load_json
from wukong_analysis.validation import bounded_int
from wukong_worker.platform.environment import safe_env
from wukong_worker.core.jobs import JobPlan
from filelock import FileLock
from wukong_worker.resources.job_ledger import JobLedger
from wukong_worker.platform.process_owner import ProcessResources


class JobManager:
    def __init__(
        self,
        root: Path,
        trusted: bool,
        isolated: bool,
    ):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.process_resources = ProcessResources()
        self.lock = FileLock(root / "jobs-admission.lock", timeout=15)
        (root / "jobs").mkdir(exist_ok=True)

    def start(self, plan: JobPlan) -> dict[str, Any]:
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

        def spawn():
            with (directory / "supervisor.log").open("ab") as log:
                return self.process_resources.spawn(
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

        return JobLedger(directory).reserve_and_spawn(spec, meta, spawn)

    def status(self, identifier):
        directory = job_dir(self.root, identifier)
        meta = JobLedger(directory).observe()
        meta["stdout"] = tail(directory / "output.stdout")
        meta["stderr"] = tail(directory / "output.stderr")
        return meta

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
        JobLedger(directory).cancel()
        return self.status(params["job_id"])

    def artifacts(self, params):
        directory = job_dir(self.root, params["job_id"])
        files = []
        for path in sorted(directory.rglob("*")):
            if (
                path.is_file()
                and not path.is_symlink()
                and path.name not in JobLedger.INTERNAL_FILES
            ):
                files.append(artifact(self.root, path))
                if len(files) == 512:
                    break
        return files
