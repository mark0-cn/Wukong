import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from wukong_worker.artifacts.common import public_file
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.core.jobs import JobOutcome
from wukong_worker.resources.job_ledger import JobLedger
from wukong_worker.resources.jobs import JobManager
from wukong_worker.resources.supervisor import supervise_job


class TraceRecoveryTests(unittest.TestCase):
    def test_supervisor_publishes_cleanup_pending(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ledger = JobLedger(root)
            with patch(
                "wukong_worker.resources.job_ledger.process_identity", return_value="creation"
            ):
                process = Mock(pid=os.getpid(), **{"stdin.write.return_value": 1})
                ledger.announce("token")
                ledger.reserve_and_spawn(
                    {"runner_token": "token"},
                    {"state": "queued", "runner_token": "token", "created_at": 0},
                    lambda: process,
                )
            with patch(
                "wukong_worker.resources.job_ledger.process_identity", return_value="creation"
            ):
                supervise_job(
                    root,
                    root,
                    lambda *_: JobOutcome(
                        "failed", {"error": "close failed"}, cleanup_pending=True
                    ),
                    io.BytesIO(b"\x01"),
                )
                observed = ledger.observe()
            self.assertEqual(observed["state"], "failed")
            self.assertTrue(observed["cleanup_pending"])
            self.assertTrue(ledger.confirm_cleanup())
            self.assertFalse(ledger.observe()["cleanup_pending"])
            self.assertFalse(ledger.confirm_cleanup())

    def test_private_journal_recovery_is_scoped_to_trace_job(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            job = root / "jobs" / ("a" * 32)
            journal = job / "trace" / "debugger" / "processes.json"
            journal.parent.mkdir(parents=True)
            journal.write_text("{}")
            atomic_json(job / "spec.json", {"kind": "trace"})
            atomic_json(
                job / "status.json",
                {"state": "lost", "cleanup_pending": True, "_ledger": {"phase": "finished"}},
            )
            with patch(
                "wukong_worker.platform.process_journal.ProcessJournal.reconcile", return_value=True
            ) as reconcile:
                manager = JobManager(root, False, False)
            self.assertEqual(reconcile.call_count, 1)
            self.assertFalse(JobLedger(job).observe()["cleanup_pending"])
            self.assertIsNotNone(manager)

    def test_failed_private_journal_recovery_keeps_pending(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            job = root / "jobs" / ("b" * 32)
            journal = job / "trace" / "debugger" / "processes.json"
            journal.parent.mkdir(parents=True)
            journal.write_text("{}")
            atomic_json(job / "spec.json", {"kind": "trace"})
            atomic_json(
                job / "status.json",
                {"state": "lost", "cleanup_pending": True, "_ledger": {"phase": "finished"}},
            )
            with patch(
                "wukong_worker.platform.process_journal.ProcessJournal.reconcile",
                return_value=False,
            ):
                JobManager(root, False, False)
            self.assertTrue(JobLedger(job).observe()["cleanup_pending"])

    def test_trace_artifacts_exclude_private_debugger_session(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            job = root / "jobs" / ("c" * 32)
            debugger = job / "trace" / "debugger"
            debugger.mkdir(parents=True)
            for path in [
                job / "trace" / "run.json",
                job / "trace" / "events.jsonl",
                debugger / "transcript.mi",
                debugger / "session.json",
                debugger / "processes.json",
            ]:
                path.write_text("{}")
            manager = JobManager.__new__(JobManager)
            manager.root = root
            manifest = manager.artifacts({"job_id": job.name})
            paths = {item["path"] for item in manifest}
            self.assertEqual(
                paths,
                {
                    f"jobs/{job.name}/trace/run.json",
                    f"jobs/{job.name}/trace/events.jsonl",
                    f"jobs/{job.name}/trace/debugger/transcript.mi",
                },
            )
            for item in manifest:
                self.assertTrue(public_file(root, item["path"]).is_file())


if __name__ == "__main__":
    unittest.main()
