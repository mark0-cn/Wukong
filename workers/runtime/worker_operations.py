"""Durable reservation and replay of external effects."""
import hashlib
import json
import time
from filelock import FileLock
from worker_common import WorkerError, atomic_json, load_json, error_kind
from worker_contract import METHODS

class OperationLedger:
    def __init__(self, root):
        self.root = root
        (root / "operations").mkdir(exist_ok=True)

    def invoke(self, method, params, execute, reconcile):
        effect = METHODS[method]["effect"]
        request_id = params.get("request_id")
        if effect == "read" or not request_id:
            return execute(method, params)
        if not isinstance(request_id, str) or not request_id or len(request_id) > 256:
            raise ValueError("request_id must be a nonempty string <=256 characters")
        fingerprint = hashlib.sha256(json.dumps({"method": method, "params": params}, sort_keys=True).encode()).hexdigest()
        ledger = self.root / "operations" / (hashlib.sha256(request_id.encode()).hexdigest() + ".json")
        # Reservation precedes the action. An owner death cannot cause an
        # uncertain debugger command or interactive write to be replayed on
        # reconnect.
        # Only duplicates of this effect wait for its execution. Cancellation,
        # shutdown and other independent effects must remain callable.
        with FileLock(str(ledger) + ".lock", timeout=305):
            if ledger.exists():
                operation = load_json(ledger)
                if operation["fingerprint"] != fingerprint:
                    raise ValueError("request_id already used for a different operation")
                if operation["state"] == "completed":
                    return reconcile(method, operation["result"])
                if operation["state"] == "failed":
                    raise WorkerError(operation.get("error_kind", "invalid_request"), operation["error"])
                if effect != "reconcile":
                    raise WorkerError("outcome_unknown", "prior operation outcome is unknown; reconcile the affected worker state instead of replaying")
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "pending", "method": method, "at": time.time()})
            try:
                result = execute(method, params)
            except Exception as error:
                atomic_json(ledger, {"fingerprint": fingerprint, "state": "failed", "error": str(error)[:4096], "error_kind": error_kind(error), "at": time.time()})
                raise
            atomic_json(ledger, {"fingerprint": fingerprint, "state": "completed", "result": result, "at": time.time()})
            return result
