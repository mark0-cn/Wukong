"""Durable execution authority under startup, observation and recovery races."""

import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.core.jobs import JobOutcome
from wukong_worker.resources.job_ledger import JobLedger
from wukong_worker.resources.supervisor import supervise_job


class JobLedgerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.ledger = JobLedger(self.root)
        self.spec = {"runner_token": "token"}
        self.meta = {"state": "queued", "runner_token": "token", "created_at": 0}
        self.identity = patch(
            "wukong_worker.resources.job_ledger.process_identity", return_value="creation"
        )
        self.identity.start()
        self.addCleanup(self.identity.stop)

    def reserve(self, spawn=None):
        self.ledger.announce("token")
        return self.ledger.reserve_and_spawn(
            self.spec, self.meta, spawn or (lambda: Mock(pid=os.getpid()))
        )

    def test_query_and_child_wait_for_parent_registration_then_claim_once(self):
        entered, release = threading.Event(), threading.Event()

        def spawn():
            entered.set()
            self.assertTrue(release.wait(5))
            return Mock(pid=os.getpid())

        parent = threading.Thread(target=lambda: self.reserve(spawn))
        parent.start()
        self.assertTrue(entered.wait(5))
        execute = Mock(return_value=JobOutcome("completed", {}))
        child = threading.Thread(target=lambda: supervise_job(self.root, self.root, execute))
        observed = []
        query = threading.Thread(target=lambda: observed.append(JobLedger(self.root).observe()))
        child.start()
        query.start()
        self.assertFalse(execute.called)
        release.set()
        for thread in [parent, child, query]:
            thread.join(5)
            self.assertFalse(thread.is_alive())
        supervise_job(self.root, self.root, execute)
        self.assertEqual(execute.call_count, 1)
        self.assertEqual(self.ledger.observe()["state"], "completed")
        self.assertNotEqual(observed[0]["state"], "lost")

    def test_failed_registration_prevents_executor_and_revokes_token(self):
        real = atomic_json

        def fail_registration(path, value):
            if value.get("_ledger", {}).get("phase") == "registered":
                raise OSError("registration failed")
            real(path, value)

        with patch("wukong_worker.resources.job_ledger.atomic_json", side_effect=fail_registration):
            with self.assertRaisesRegex(OSError, "registration failed"):
                self.reserve()
        execute = Mock()
        supervise_job(self.root, self.root, execute)
        execute.assert_not_called()
        self.assertTrue(self.ledger.observe()["cleanup_pending"])

    def test_legacy_or_reserved_without_pid_is_uncertain_and_cannot_claim(self):
        for internal in [{}, {"_ledger": {"schema": 1, "phase": "reserved", "version": 1}}]:
            atomic_json(self.ledger.path, {**self.meta, **internal})
            observed = self.ledger.observe()
            self.assertEqual(observed["state"], "lost")
            self.assertTrue(observed["cleanup_pending"])
            self.assertFalse(observed["cleanup_confirmed"])
            self.assertIsNone(self.ledger.claim("token", os.getpid()))

    def test_late_finish_never_overwrites_lost_after_process_identity_changes(self):
        self.reserve()
        version = self.ledger.claim("token", os.getpid())
        with patch("wukong_worker.resources.job_ledger.process_identity", return_value="missing"):
            self.assertEqual(self.ledger.observe()["state"], "lost")
        self.assertFalse(self.ledger.finish("token", version, "completed", result={}))
        self.assertEqual(self.ledger.observe()["state"], "lost")

    def test_wrong_token_version_and_duplicate_claim_are_rejected(self):
        self.reserve()
        self.assertIsNone(self.ledger.claim("wrong", os.getpid()))
        version = self.ledger.claim("token", os.getpid())
        self.assertIsNotNone(version)
        self.assertIsNone(self.ledger.claim("token", os.getpid()))
        self.assertFalse(self.ledger.finish("token", version + 1, "completed"))
        self.assertTrue(JobLedger(self.root).finish("token", version, "completed"))
        self.assertFalse(self.ledger.finish("token", version, "failed"))

    def test_cancel_before_claim_and_snapshot_import_never_execute(self):
        self.reserve()
        self.ledger.cancel()
        self.assertIsNone(self.ledger.claim("token", os.getpid()))
        self.assertEqual(self.ledger.observe()["state"], "cancelled")
        self.ledger.import_snapshot()
        self.assertEqual(self.ledger.observe()["state"], "cancelled")
        self.assertIsNone(self.ledger.claim("token", os.getpid()))
        self.assertNotIn("runner_token", load_json(self.ledger.path))

    def test_import_active_revokes_authority_and_hides_internal_metadata(self):
        self.reserve()
        self.ledger.import_snapshot()
        value = self.ledger.observe()
        self.assertEqual(value["state"], "lost")
        self.assertFalse(value["cleanup_pending"])
        self.assertFalse(JobLedger.PRIVATE.intersection(value))
        self.assertIsNone(self.ledger.claim("token", os.getpid()))
