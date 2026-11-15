"""Management probes must not take ownership of the running worker's state."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from wukong_worker.workbench.control import doctor


class WorkbenchDoctorTests(unittest.TestCase):
    def test_inventory_uses_real_handlers_without_recovering_live_sessions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            session = root / "interact" / ("a" * 32) / "session.json"
            session.parent.mkdir(parents=True)
            contents = json.dumps({"session_id": "a" * 32, "state": "running"}).encode()
            session.write_bytes(contents)
            paths_before = sorted(str(path.relative_to(root)) for path in root.rglob("*"))
            with (
                patch(
                    "wukong_worker.workbench.control.subprocess.run",
                    return_value=subprocess.CompletedProcess([], 0, b"probe\n", b""),
                ),
                patch.dict("os.environ", {"WUKONG_GHIDRA_HEADLESS": ""}),
            ):
                result = doctor(root)
            self.assertFalse(result["passed"])
            self.assertFalse(result["checks"]["ghidra"]["passed"])
            self.assertIn("job.start", result["inventory"]["methods"])
            self.assertIn("debug.start", result["inventory"]["methods"])
            self.assertTrue(result["inventory"]["capabilities"]["process_jobs"])
            self.assertEqual(result["inventory"]["worker_root"], str(root))
            self.assertEqual(session.read_bytes(), contents)
            self.assertEqual(
                sorted(str(path.relative_to(root)) for path in root.rglob("*")), paths_before
            )
