"""Cleanup requires durable ownership and confirmation, including restart gaps."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from wukong_worker.platform.process_journal import ProcessJournal
from wukong_worker.platform.process_owner import ProcessResources
from wukong_worker.resources.sessions import SessionManager

MODULE = "wukong_worker.platform.process_journal."


class ProcessJournalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "processes.json"
        self.boot = patch(MODULE + "boot_identity", return_value="boot")
        self.namespace = patch(MODULE + "os.readlink", return_value="namespace", create=True)
        self.boot.start()
        self.namespace.start()
        self.addCleanup(self.boot.stop)
        self.addCleanup(self.namespace.stop)

    def live(self):
        journal = ProcessJournal(self.path)
        with patch(
            "wukong_worker.platform.process_control.process_identity", return_value="original"
        ):
            journal.spawned(type("Process", (), {"pid": 1234})())
        return journal

    def test_uncertain_spawn_stays_pending_across_retries(self):
        ProcessJournal(self.path)
        with patch(MODULE + "os.killpg", create=True) as kill:
            for _ in range(2):
                self.assertFalse(ProcessJournal.reconcile(self.path))
                self.assertEqual(json.loads(self.path.read_text())["observed"], "recovery_required")
            kill.assert_not_called()

    def test_rejected_spawn_can_close_without_hiding_original_error(self):
        journal = ProcessJournal(self.path)
        journal.starting()
        journal.spawn_failed()
        self.assertTrue(ProcessJournal.reconcile(self.path))

    def test_reused_pid_is_never_signalled(self):
        self.live()
        with (
            patch(
                "wukong_worker.platform.process_control.process_identity",
                return_value="replacement",
            ),
            patch(MODULE + "group_members", return_value=[1234]),
            patch(MODULE + "os.killpg", create=True) as kill,
        ):
            self.assertFalse(ProcessJournal.reconcile(self.path))
            kill.assert_not_called()

    def test_scratch_failure_never_confirms_closed(self):
        self.live()
        with (
            patch(
                "wukong_worker.platform.process_control.process_identity", return_value="missing"
            ),
            patch(MODULE + "group_members", return_value=[]),
            patch(MODULE + "remove_scratch", side_effect=OSError("busy")),
        ):
            with self.assertRaises(OSError):
                ProcessJournal.reconcile(self.path)
        self.assertEqual(json.loads(self.path.read_text())["observed"], "recovery_required")

    def test_startup_finds_journal_without_session_receipt(self):
        root = Path(self.temp.name)
        self.path = root / "debug" / ("a" * 32) / "processes.json"
        self.path.parent.mkdir(parents=True)
        self.live()
        with (
            patch(
                "wukong_worker.platform.process_control.process_identity", return_value="missing"
            ),
            patch(MODULE + "group_members", return_value=[]),
        ):
            SessionManager(root, True, False)
        state = json.loads(self.path.with_name("session.json").read_text())
        self.assertEqual(state["state"], "lost")
        self.assertTrue(state["cleanup_confirmed"])


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux process ownership")
class LiveProcessJournalTests(unittest.TestCase):
    def test_restart_reconciles_owned_group_and_live_close_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "processes.json"
            owner = ProcessResources(path)
            process = owner.spawn(
                [sys.executable, "-c", "import time; time.sleep(300)"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                self.assertTrue(ProcessJournal.reconcile(path))
                owner.close()
                self.assertIsNotNone(process.returncode)
                self.assertFalse(Path(f"/proc/{process.pid}").exists())
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, 9)
                    process.wait()
