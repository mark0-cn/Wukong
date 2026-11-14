"""External-effect receipts, uncertainty and independent control requests."""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import tempfile
import threading
import unittest
from worker_common import WorkerError, atomic_json
from worker_operations import OperationLedger


class OperationTests(unittest.TestCase):
    def test_completed_python_request_is_not_executed_twice(self):
        with tempfile.TemporaryDirectory() as directory:
            ledger = OperationLedger(Path(directory))
            calls = []
            def execute(method, params):
                calls.append(method)
                return {"job_id": "original"}
            params = {"request_id": "stable", "source": "print(1)"}
            first = ledger.invoke("py.run", params, execute, lambda _, result: result)
            self.assertEqual(first, ledger.invoke("py.run", params, execute, lambda _, result: result))
            self.assertEqual(calls, ["py.run"])
            with self.assertRaises(ValueError):
                ledger.invoke("py.run", {**params, "source": "print(2)"}, execute, lambda _, result: result)

    def test_uncertain_python_request_is_not_replayed(self):
        with tempfile.TemporaryDirectory() as directory:
            ledger = OperationLedger(Path(directory))
            params = {"request_id": "uncertain", "source": "print(1)"}
            fingerprint = hashlib.sha256(json.dumps({"method": "py.run", "params": params}, sort_keys=True).encode()).hexdigest()
            path = Path(directory) / "operations" / (hashlib.sha256(b"uncertain").hexdigest() + ".json")
            atomic_json(path, {"fingerprint": fingerprint, "state": "pending"})
            with self.assertRaises(WorkerError) as error:
                ledger.invoke("py.run", params, lambda *_: self.fail("uncertain effect replayed"), lambda _, result: result)
            self.assertEqual(error.exception.kind, "outcome_unknown")

    def test_busy_effect_does_not_block_cancellation(self):
        with tempfile.TemporaryDirectory() as directory, ThreadPoolExecutor(2) as pool:
            ledger = OperationLedger(Path(directory))
            started, release = threading.Event(), threading.Event()
            def execute(*_):
                started.set()
                if not release.wait(5):
                    raise TimeoutError("test did not release effect")
                return {"result": "completed"}
            busy = pool.submit(ledger.invoke, "exec.run", {"request_id": "busy"}, execute, lambda _, result: result)
            self.assertTrue(started.wait(5))
            try:
                cancel = pool.submit(ledger.invoke, "job.cancel", {"request_id": "cancel"}, lambda *_: {"cancelled": True}, lambda _, result: result)
                self.assertEqual(cancel.result(timeout=2), {"cancelled": True})
            finally:
                release.set()
                busy.result(timeout=5)
