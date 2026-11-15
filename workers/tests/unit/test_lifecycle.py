"""Shared semantic corpus runs against Python rules and durable effect ownership."""

import json
from pathlib import Path
import tempfile
import unittest
from wukong_worker.core.lifecycle import ResourceObservation
from wukong_worker.resources.operations import OperationLedger
from wukong_worker.core.errors import WorkerError
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.host.runtime import Runtime

CORPUS = json.loads(
    (Path(__file__).resolve().parents[2] / "protocol/fixtures/lifecycle.json").read_text()
)


class InterruptedOwner(BaseException):
    pass


class LifecycleTests(unittest.TestCase):
    def test_shared_completion_and_uncertainty_observations(self):
        for case in CORPUS["observations"]:
            with self.subTest(case=case["name"]):
                observation = getattr(ResourceObservation, case["kind"])(case["wire"])
                self.assertEqual(observation.settled, case["settled"])
                self.assertEqual(observation.outcome, case["outcome"])

    def test_shared_durable_effect_scenarios(self):
        for case in CORPUS["effects"]:
            with self.subTest(case=case["name"]), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                ledger = OperationLedger(root)
                params = {"request_id": "stable", "target": "files/fixture"}
                calls = []
                result = {"session_id": "a" * 32, "state": "running"}

                def execute(*_):
                    calls.append(1)
                    if case["interrupt"]:
                        raise InterruptedOwner()
                    return result

                if case["interrupt"]:
                    with self.assertRaises(InterruptedOwner):
                        ledger.invoke("debug.start", params, execute, lambda _, reply: reply)
                else:
                    ledger.invoke("debug.start", params, execute, lambda _, reply: reply)
                ledger = OperationLedger(root)
                if case["changed"]:
                    params = {**params, "target": "files/different"}

                def reconcile(_, reply):
                    return {**reply, "state": "closed"} if case["closed"] else reply

                if case["expect"] == "reuse":
                    replayed = ledger.invoke("debug.start", params, execute, reconcile)
                    self.assertEqual(replayed, reconcile(None, result))
                elif case["expect"] == "unknown":
                    with self.assertRaises(WorkerError) as error:
                        ledger.invoke("debug.start", params, execute, reconcile)
                    self.assertEqual(error.exception.kind, "outcome_unknown")
                else:
                    with self.assertRaisesRegex(ValueError, "different operation"):
                        ledger.invoke("debug.start", params, execute, reconcile)
                self.assertEqual(len(calls), 1)

    def test_shared_invalid_lifecycle_fields(self):
        for case in CORPUS["invalid_observations"]:
            with self.subTest(case=case["name"]), self.assertRaises(ValueError):
                getattr(ResourceObservation, case["kind"])(case["wire"])

    def test_shutdown_and_maintenance_share_unconfirmed_resource_gate(self):
        from wukong_worker.workbench.control import idle

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "debug" / ("a" * 32) / "session.json"
            path.parent.mkdir(parents=True)
            atomic_json(path, {"state": "running"})
            runtime = Runtime(root, True)
            self.assertFalse(idle(root)["idle"])
            with self.assertRaisesRegex(ValueError, "cleanup remains unconfirmed"):
                runtime.shutdown({})
            self.assertFalse(runtime.stopping)
            atomic_json(path, {"state": "lost", "cleanup_confirmed": True})
            self.assertTrue(idle(root)["idle"])
            self.assertEqual(runtime.shutdown({}), {"state": "shutdown"})
