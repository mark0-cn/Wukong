"""Job planning keeps optional request identities optional without executing targets."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from wukong_worker.resources.jobs import JobManager
from wukong_worker.operations.jobs import JobService


class FuzzPlanningTests(unittest.TestCase):
    def test_missing_request_id_allocates_distinct_campaigns_and_explicit_ids_remain_stable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "files/seeds").mkdir(parents=True)
            (root / "files/fixture").write_bytes(b"trusted inert fixture: never executed")
            (root / "files/seeds/seed").write_bytes(b"input")
            manager = JobService(
                root,
                trusted=True,
                isolated=False,
                jobs=JobManager(root, trusted=True, isolated=False),
            )
            params = {"target": "files/fixture", "seeds": "files/seeds"}
            with (
                patch.object(manager, "doctor", return_value={"capabilities": {"fuzz_qemu": True}}),
                patch.object(manager, "start_job", side_effect=lambda spec, kind: spec),
            ):
                first = manager.fuzz(params)
                second = manager.fuzz(params)
                self.assertNotEqual(first["afl_output"], second["afl_output"])
                stable = manager.fuzz({**params, "request_id": "stable-campaign"})
                self.assertEqual(
                    stable["afl_output"],
                    manager.fuzz({**params, "request_id": "stable-campaign"})["afl_output"],
                )
            for spec in [first, second, stable]:
                self.assertTrue((root / spec["afl_output"]).is_dir())
                self.assertEqual(spec["argv"][:2], ["afl-fuzz", "-Q"])


class PythonSourceTests(unittest.TestCase):
    def test_file_source_is_bounded_and_sealed_by_content(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "files").mkdir()
            source = root / "files/source.py"
            source.write_bytes("print('你好')\n".encode("utf-8"))
            manager = JobService(
                root,
                trusted=True,
                isolated=False,
                jobs=JobManager(root, trusted=True, isolated=False),
            )
            with patch.object(manager, "start_job", side_effect=lambda spec: spec):
                first = manager.python({"source_file": "files/source.py"})
                self.assertEqual(first, manager.python({"source_file": "files/source.py"}))
                self.assertEqual((root / first["argv"][1]).read_bytes(), source.read_bytes())
                source.write_bytes(b"x" * 262145)
                with self.assertRaisesRegex(ValueError, "256 KiB"):
                    manager.python({"source_file": "files/source.py"})
                with self.assertRaises(ValueError):
                    manager.python({"source": "你" * 100000})
                with self.assertRaises(ValueError):
                    manager.python({"source": "pass", "source_file": "files/source.py"})
