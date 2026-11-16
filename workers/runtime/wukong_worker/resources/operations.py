"""Durable reservation and replay of external effects."""

import hashlib
import json
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any
from filelock import FileLock
from wukong_worker.core.errors import WorkerError, error_kind
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.core.contract import METHODS


class OperationLedger:
    def __init__(self, root: Path) -> None:
        self.root = root
        (root / "operations").mkdir(exist_ok=True)

    def invoke(
        self,
        method: str,
        params: dict[str, Any],
        execute: Callable[[str, dict[str, Any]], Any],
        reconcile: Callable[[str, Any], Any],
    ) -> Any:
        effect = METHODS[method]["effect"]
        request_id = params.get("request_id")
        if effect == "read" or not request_id:
            return execute(method, params)
        if not isinstance(request_id, str) or not request_id or len(request_id) > 256:
            raise ValueError("request_id must be a nonempty string <=256 characters")
        fingerprint = hashlib.sha256(
            json.dumps({"method": method, "params": params}, sort_keys=True).encode()
        ).hexdigest()
        ledger = (
            self.root / "operations" / (hashlib.sha256(request_id.encode()).hexdigest() + ".json")
        )
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
                # Only owned cleanup may retry a failed reconciliation with the
                # original request. Never extend this exception to starts or
                # once-only target actions, even if their contract is reconcile.
                cleanup_retry = method in {
                    "debug.close",
                    "interact.close",
                    "job.cancel",
                    "fuzz.stop",
                }
                if operation["state"] == "failed" and not cleanup_retry:
                    raise WorkerError(
                        operation.get("error_kind", "invalid_request"), operation["error"]
                    )
                if effect != "reconcile":
                    raise WorkerError(
                        "outcome_unknown",
                        "prior operation outcome is unknown; reconcile the affected worker state instead of replaying",
                    )
            atomic_json(
                ledger,
                {
                    "fingerprint": fingerprint,
                    "state": "pending",
                    "method": method,
                    "at": time.time(),
                },
            )
            try:
                result = execute(method, params)
            except Exception as error:
                # Record all execution failures at the durable boundary. Narrowing
                # this catch would leave avoidable ambiguity after ordinary errors.
                atomic_json(
                    ledger,
                    {
                        "fingerprint": fingerprint,
                        "state": "failed",
                        "error": str(error)[:4096],
                        "error_kind": error_kind(error),
                        "at": time.time(),
                    },
                )
                raise
            atomic_json(
                ledger,
                {
                    "fingerprint": fingerprint,
                    "state": "completed",
                    "result": result,
                    "at": time.time(),
                },
            )
            return result
