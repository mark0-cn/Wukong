import tempfile
import unittest
from pathlib import Path

from worker_common import atomic_json
from worker_jobs import JobManager


class JobInventoryTests(unittest.TestCase):
    def test_cursor_pages_preserve_every_terminal_job_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manager = JobManager(root, trusted=True, isolated=False)
            ids = [f"{i:032x}" for i in range(70)]
            for identifier in reversed(ids):
                job = root / "jobs" / identifier
                job.mkdir()
                atomic_json(job / "status.json", dict(job_id=identifier, state="completed", kind="process"))
            first = manager.list({})
            second = manager.list(dict(after=first[-1]["job_id"]))
            self.assertEqual(len(first), 64)
            self.assertEqual([row["job_id"] for row in first + second], ids)
            self.assertEqual(manager.list(dict(after=ids[-1])), [])
            self.assertEqual([row["job_id"] for row in manager.list(dict(after=ids[0], limit=2))], ids[1:3])
            for limit in [0, 65]:
                with self.assertRaises(ValueError):
                    manager.list(dict(limit=limit))
