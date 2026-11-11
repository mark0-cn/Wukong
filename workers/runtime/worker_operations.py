"""Durable reservation and replay of external effects."""
import hashlib
import json
import threading
import time
from worker_common import WorkerError, atomic_json, load_json, rpc_error

class OperationLedger:
    def __init__(self, root):
        self.root = root
        self.lock = threading.RLock()
        (root / "operations").mkdir(exist_ok=True)

    def invoke(self, method, params, execute, reconcile):
        effects = {"runtime.prepare", "job.start", "fuzz.start", "debug.start", "debug.command", "debug.control", "debug.close",
                   "file.put", "crash.replay", "input.minimize", "poc.verify", "job.cancel", "fuzz.stop",
                   "interact.start", "interact.write", "interact.close", "exec.run",
                   "file.upload.begin", "file.upload.write", "file.upload.commit"}
        request_id = params.get("request_id")
        if method not in effects or not request_id:
            return execute(method, params)
        if not isinstance(request_id, str) or not request_id or len(request_id) > 256:
            raise ValueError("request_id must be a nonempty string <=256 characters")
        fingerprint = hashlib.sha256(json.dumps({"method": method, "params": params}, sort_keys=True).encode()).hexdigest()
        ledger = self.root / "operations" / (hashlib.sha256(request_id.encode()).hexdigest() + ".json")
        # Reservation precedes the action. An owner death cannot cause an
        # uncertain debugger command or interactive write to be replayed on
        # reconnect.
        with self.lock:
            if ledger.exists():
                operation = load_json(ledger)
                if operation["fingerprint"] != fingerprint:
                    raise ValueError("request_id already used for a different operation")
                if operation["state"] == "completed":
                    return reconcile(method, operation["result"])
                if operation["state"] == "failed":
                    raise WorkerError(operation.get("error_kind", "invalid_request"), operation["error"])
                if method not in {"runtime.prepare", "job.start", "fuzz.start", "file.put", "crash.replay", "input.minimize", "poc.verify", "job.cancel", "fuzz.stop", "debug.close",
                                  "file.upload.begin", "file.upload.write", "file.upload.commit"}:
                    raise WorkerError("outcome_unknown", "prior operation outcome is unknown; reconcile the affected worker state instead of replaying")
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "pending", "method": method, "at": time.time()})
            try:
                result = execute(method, params)
            except Exception as error:
                atomic_json(ledger, {"fingerprint": fingerprint, "state": "failed", "error": str(error)[:4096], "error_kind": rpc_error(error)["data"]["kind"], "at": time.time()})
                raise
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "completed", "result": result, "at": time.time()})
            return result
