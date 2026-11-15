"""A process job uses the shared LaunchSpec without per-call launch overrides."""

import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, Mock
from wukong_worker.resources.jobs import JobManager


class ProcessLaunchPlanningTests(unittest.TestCase):
    def test_normalized_request_fingerprint_and_duplicate_do_not_depend_on_runner_token(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manager = JobManager(root, True, False)
            params = {
                "argv": ["fixture", "arg"],
                "request_id": "job-admission-fixture",
                "timeout_secs": None,
                "output_limit": None,
            }
            original = json.loads(json.dumps(params))
            with patch(
                "wukong_worker.resources.jobs.subprocess.Popen", return_value=Mock(pid=123)
            ) as spawn:
                first = manager.start_job(params)
                duplicate = manager.start_job(params)
                self.assertEqual(first["job_id"], duplicate["job_id"])
                self.assertEqual(spawn.call_count, 1)
                self.assertEqual(params, original)
                meta = json.loads((root / "jobs" / first["job_id"] / "status.json").read_text())
                self.assertEqual(
                    meta["fingerprint"],
                    "b9175accce21c0366f487d7a49f676ccdbde5e826e2907fe2556cbdae04e5b9c",
                )
                self.assertNotIn("runner_token", first)
                with self.assertRaisesRegex(ValueError, "different request"):
                    manager.start_job({**params, "argv": ["different"]})
                self.assertEqual(spawn.call_count, 1)

    def test_failed_spawn_keeps_reservation_and_retry_never_reexecutes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manager = JobManager(root, True, False)
            params = {"argv": ["fixture"], "request_id": "spawn-failure"}
            with patch(
                "wukong_worker.resources.jobs.subprocess.Popen",
                side_effect=OSError("fixture spawn failed"),
            ) as spawn:
                with patch("wukong_worker.resources.jobs.time.time", return_value=100):
                    with self.assertRaisesRegex(OSError, "fixture spawn failed"):
                        manager.start_job(params)
                with patch("wukong_worker.resources.jobs.time.time", return_value=111):
                    retry = manager.start_job(params)
                self.assertEqual(spawn.call_count, 1)
                self.assertEqual(retry["state"], "lost")
                self.assertIn("never created", retry["error"])
                self.assertEqual(len(list((root / "jobs").iterdir())), 1)

    def test_launch_only_job_records_manifest_timeout_and_rejects_overrides_before_spawn(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = root / "files/case"
            base.mkdir(parents=True)
            (base / "target").write_bytes(b"inert fixture, never executed")
            (base / "input.bin").write_bytes(b"data")
            spec = dict(
                schema_version=1,
                base="files/case",
                target="target",
                files={"target": hashlib.sha256((base / "target").read_bytes()).hexdigest()},
                args=["@@"],
                cwd=".",
                input_mode="argv_file",
                env={},
                timeout_secs=5,
            )
            (base / "launch.json").write_text(json.dumps(spec))
            manager = JobManager(root, True, False)
            params = dict(launch_spec="files/case/launch.json", input="files/case/input.bin")
            with patch(
                "wukong_worker.resources.jobs.subprocess.Popen", return_value=Mock(pid=123)
            ) as spawn:
                result = manager.start_job(params)
                saved = json.loads((root / "jobs" / result["job_id"] / "spec.json").read_text())
                self.assertEqual(saved["timeout_secs"], 5)
                self.assertEqual(saved["launch_spec"], params["launch_spec"])
                self.assertEqual(spawn.call_count, 1)
                for override in [
                    dict(argv=["other"]),
                    dict(env={"AFL_X": "1"}),
                    dict(cwd="files/case"),
                    dict(timeout_secs=6),
                ]:
                    with self.assertRaisesRegex(ValueError, "LaunchSpec owns"):
                        manager.start_job({**params, **override})
                self.assertEqual(spawn.call_count, 1)


if __name__ == "__main__":
    unittest.main()
