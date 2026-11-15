"""Jobs."""

from pathlib import Path
from typing import Any
import hashlib
import subprocess
import sys
import threading
import time
import uuid
from wukong_worker import BRIDGE_PATH
from wukong_worker.artifacts.common import artifact, job_dir, tail
from wukong_worker.core.lifecycle import TERMINAL
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_analysis.validation import bounded_int
from wukong_worker.platform.environment import safe_env
from wukong_worker.core.jobs import JobPlan
from wukong_worker.platform.process_control import process_identity
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
        self.lock = threading.RLock()
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
