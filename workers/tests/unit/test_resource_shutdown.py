"""Centralized worker shutdown: read-only inventory, admission and cleanup rules."""

import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.host.runtime import Runtime
from wukong_worker.platform.process_control import process_identity
from wukong_worker.resources.inventory import pending_resources, records
from wukong_worker.workbench.control import idle

JOURNAL = "wukong_worker.platform.process_journal."


def session_fixture(root, kind, identifier, state):
    path = root / kind / identifier / "session.json"
    path.parent.mkdir(parents=True)
    atomic_json(path, state)
    return path


class ResourceInventoryTests(unittest.TestCase):
    def test_process_journal_without_session_receipt_is_unsettled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            identifier = "a" * 32
            journal = root / "debug" / identifier / "processes.json"
            journal.parent.mkdir(parents=True)
            contents = {"desired": "live", "observed": "live", "children": []}
            atomic_json(journal, contents)
            self.assertEqual(
                pending_resources(root),
                [{"kind": "debug", "id": identifier, "state": "recovery_required"}],
            )
            # Enumeration is read-only: it neither publishes a receipt nor
            # rewrites the interrupted journal it discovered.
            self.assertFalse((journal.parent / "session.json").exists())
            self.assertEqual(json.loads(journal.read_text()), contents)

    def test_job_enumeration_never_publishes_a_status_transition(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            job = root / "jobs" / ("b" * 32)
            job.mkdir(parents=True)
            status = job / "status.json"
            original = json.dumps({"state": "queued", "kind": "process", "created_at": 0}).encode()
            status.write_bytes(original)
            self.assertFalse(idle(root)["idle"])
            self.assertEqual([record.kind for record in records(root)], ["job"])
            self.assertEqual(status.read_bytes(), original)


class ShutdownAdmissionTests(unittest.TestCase):
    def test_shutdown_refuses_active_job_and_surfaces_the_reason(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            job = root / "jobs" / ("c" * 32)
            job.mkdir(parents=True)
            atomic_json(
                job / "status.json",
                {
                    "job_id": job.name,
                    "kind": "process",
                    "state": "queued",
                    "created_at": 0,
                    "runner_token": "fixture-token",
                    "_ledger": {"schema": 1, "phase": "registered", "version": 1},
                    "supervisor_pid": os.getpid(),
                    "supervisor_identity": process_identity(os.getpid()),
                },
            )
            runtime = Runtime(root, trusted=True)
            self.assertFalse(idle(root)["idle"])
            with self.assertRaisesRegex(ValueError, "jobs are active"):
                runtime.shutdown({})
            self.assertFalse(runtime.stopping)
            self.assertEqual(
                json.loads((job / "status.json").read_text())["state"],
                "queued",
                "admission must not publish a transition for an active job",
            )

    def test_inventory_display_and_shutdown_admission_share_the_judgement(self):
        # Both fixtures are created before the runtime, so display and admission
        # observe the same startup reconciliation and durable records.
        cases = [
            ("unconfirmed_session", "debug", {"state": "running"}, False),
            ("confirmed_journal_only", "interact", None, True),
        ]
        for name, kind, state, admitted in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                identifier = "a" * 32
                if state is None:
                    journal = root / kind / identifier / "processes.json"
                    journal.parent.mkdir(parents=True)
                    atomic_json(journal, {"observed": "closed"})
                else:
                    session_fixture(root, kind, identifier, state)
                runtime = Runtime(root, trusted=True)
                self.assertEqual(idle(root)["idle"], admitted)
                self.assertEqual(bool(pending_resources(root)), not admitted)
                if admitted:
                    self.assertEqual(runtime.shutdown({}), {"state": "shutdown"})
                    self.assertTrue(runtime.stopping)
                else:
                    with self.assertRaisesRegex(ValueError, "cleanup remains unconfirmed"):
                        runtime.shutdown({})
                    self.assertFalse(runtime.stopping)


class ShutdownCleanupTests(unittest.TestCase):
    def test_failed_cleanup_retains_ownership_and_closes_other_sessions(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            failed, other = Mock(id="a" * 32), Mock(id="b" * 32)
            failed.close.side_effect = RuntimeError("fixture cleanup failure")
            other.close.return_value = {"session_id": other.id, "state": "closed"}
            runtime.sessions.debug[failed.id] = failed
            runtime.sessions.interact[other.id] = other
            with self.assertRaisesRegex(RuntimeError, "fixture cleanup failure"):
                runtime.shutdown({})
            self.assertIs(runtime.sessions.debug[failed.id], failed)
            self.assertFalse(runtime.stopping)
            other.close.assert_called_once_with()
            failed.close.side_effect = None
            failed.close.return_value = {"session_id": failed.id, "state": "closed"}
            self.assertEqual(runtime.shutdown({}), {"state": "shutdown"})

    def test_unconfirmed_record_stays_blocking_while_another_is_reconciled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            blocked = session_fixture(root, "debug", "a" * 32, {"state": "running"})
            journal = root / "interact" / ("b" * 32) / "processes.json"
            journal.parent.mkdir(parents=True)
            atomic_json(journal, {"observed": "closed"})
            runtime = Runtime(root, trusted=True)
            blocked_record = json.loads(blocked.read_text())
            self.assertEqual(blocked_record["state"], "recovery_required")
            self.assertFalse(blocked_record["cleanup_confirmed"])
            cleaned = json.loads((journal.parent / "session.json").read_text())
            self.assertEqual(cleaned["state"], "lost")
            self.assertTrue(cleaned["cleanup_confirmed"])
            with self.assertRaisesRegex(ValueError, "cleanup remains unconfirmed"):
                runtime.shutdown({})
            # The refusal preserves the original blocking record; the other
            # leftover resource was still confirmed by the same reconciliation.
            self.assertEqual(json.loads(blocked.read_text()), blocked_record)
            self.assertEqual(
                pending_resources(root),
                [{"kind": "debug", "id": "a" * 32, "state": "recovery_required"}],
            )


class RestartReconciliationTests(unittest.TestCase):
    def test_restart_and_shutdown_use_the_same_record_rules(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            journal = root / "interact" / ("c" * 32) / "processes.json"
            journal.parent.mkdir(parents=True)
            atomic_json(journal, {"observed": "closed"})
            runtime = Runtime(root, trusted=True)
            state = json.loads((journal.parent / "session.json").read_text())
            self.assertEqual(state["state"], "lost")
            self.assertTrue(state["cleanup_confirmed"])
            self.assertTrue(idle(root)["idle"])
            self.assertEqual(runtime.shutdown({}), {"state": "shutdown"})

    def test_explicit_close_discovers_an_unpublished_session(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            identifier = "d" * 32
            journal = root / "interact" / identifier / "processes.json"
            journal.parent.mkdir(parents=True)
            atomic_json(journal, {"observed": "closed"})
            runtime = Runtime(root, trusted=True)
            # An interrupted startup may leave only the process journal; explicit
            # close must discover it exactly like startup reconciliation does.
            receipt = journal.parent / "session.json"
            receipt.unlink()
            result = runtime.sessions.interact_close({"session_id": identifier})
            self.assertEqual(
                result, {"session_id": identifier, "state": "lost", "cleanup_confirmed": True}
            )
            self.assertEqual(pending_resources(root), [])

    def test_uncertain_restart_cleanup_keeps_the_blocking_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "interact" / ("e" * 32) / "processes.json"
            path.parent.mkdir(parents=True)
            with (
                patch(JOURNAL + "boot_identity", return_value="boot"),
                patch(JOURNAL + "os.readlink", return_value="namespace", create=True),
            ):
                from wukong_worker.platform.process_journal import ProcessJournal

                ProcessJournal(path)
                runtime = Runtime(root, trusted=True)
            state = json.loads((path.parent / "session.json").read_text())
            self.assertEqual(state["state"], "recovery_required")
            self.assertFalse(state["cleanup_confirmed"])
            self.assertEqual(json.loads(path.read_text())["observed"], "recovery_required")
            self.assertFalse(idle(root)["idle"])
            with self.assertRaisesRegex(ValueError, "cleanup remains unconfirmed"):
                runtime.shutdown({})

    def test_restart_never_cleans_under_changed_process_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "debug" / ("f" * 32) / "processes.json"
            path.parent.mkdir(parents=True)
            with (
                patch(JOURNAL + "boot_identity", return_value="boot"),
                patch(JOURNAL + "os.readlink", return_value="namespace", create=True),
                patch(
                    "wukong_worker.platform.process_control.process_identity",
                    return_value="original",
                ),
            ):
                from wukong_worker.platform.process_journal import ProcessJournal

                journal = ProcessJournal(path)
                journal.spawned(type("Process", (), {"pid": 1234})())
            with (
                patch(JOURNAL + "boot_identity", return_value="boot"),
                patch(JOURNAL + "os.readlink", return_value="namespace", create=True),
                patch(
                    "wukong_worker.platform.process_control.process_identity",
                    return_value="replacement",
                ),
                patch(JOURNAL + "group_members", return_value=[1234]),
            ):
                runtime = Runtime(root, trusted=True)
            state = json.loads((path.parent / "session.json").read_text())
            self.assertEqual(state["state"], "recovery_required")
            self.assertFalse(state["cleanup_confirmed"])
            self.assertFalse(idle(root)["idle"])
            with self.assertRaisesRegex(ValueError, "cleanup remains unconfirmed"):
                runtime.shutdown({})


if __name__ == "__main__":
    unittest.main()
