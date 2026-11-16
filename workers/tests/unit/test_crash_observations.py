"""Regression checks for crash evidence, not an exploit evaluation suite."""

import base64
import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from wukong_worker.artifacts.metadata import atomic_json, load_json
from wukong_worker.debugger.crash import CrashService
from wukong_analysis.crash import location_signature, normalize_address
from wukong_worker.resources.supervisor import supervise_job
from wukong_worker.resources.job_ledger import JobLedger
from unittest.mock import Mock
from wukong_worker.operations.executors import execute_job


class CrashTests(unittest.TestCase):
    def test_signature_tracks_code_identity_and_offset_not_aslr(self):
        def snapshot(base, offset=16, digest="one"):
            profile = {
                "maps": [
                    {
                        "start": base,
                        "end": base + 4096,
                        "offset": 4096,
                        "inode": 3,
                        "device": [8, 1],
                    }
                ],
                "modules": [{"inode": 3, "device": [8, 1], "sha256": digest}],
            }
            return {"signal": "SIGSEGV", "pc_location": normalize_address(base + offset, profile)}

        self.assertEqual(location_signature(snapshot(4096)), location_signature(snapshot(8192)))
        self.assertNotEqual(
            location_signature(snapshot(4096)), location_signature(snapshot(4096, 17))
        )
        self.assertNotEqual(
            location_signature(snapshot(4096)), location_signature(snapshot(4096, digest="two"))
        )
        self.assertIsNone(location_signature(snapshot(4096, digest=None)))

    def test_exact_pattern_width_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "pattern").write_bytes(b"aaaabaaacaaa")
            with self.assertRaisesRegex(ValueError, "exactly n"):
                CrashService(root, True, False).cyclic_offset(
                    {
                        "pattern": "pattern",
                        "observed_base64": base64.b64encode(b"aaaabaaa").decode(),
                        "alphabet": "abcd",
                        "n": 4,
                    }
                )

    def minimize(self, strong, weak=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            job = root / "job"
            job.mkdir()
            (root / "input").write_bytes(b"xxAyy")
            atomic_json(
                job / "spec.json",
                {
                    "kind": "minimize",
                    "runner_token": "test",
                    "input": "input",
                    "max_attempts": 40,
                    "minimize_timeout_secs": 5,
                    "allow_signal_only": weak,
                },
            )
            with patch(
                "wukong_worker.resources.job_ledger.process_identity", return_value="fixture"
            ):
                JobLedger(job).announce("test")
                JobLedger(job).reserve_and_spawn(
                    load_json(job / "spec.json"),
                    {"state": "queued", "runner_token": "test"},
                    lambda: Mock(pid=os.getpid(), **{"stdin.write.return_value": 1}),
                )

            def replay(root, directory, spec, data, index):
                result = {"state": "completed", "signal": 11, "exit_code": -11}
                if strong:
                    result["location_signature"] = "location:" + ("A" if b"A" in data else "other")
                return result

            with (
                patch("wukong_worker.operations.reduce_job.replay", side_effect=replay),
                patch(
                    "wukong_worker.resources.job_ledger.process_identity", return_value="fixture"
                ),
            ):
                supervise_job(root, job, execute_job, io.BytesIO(b"\x01"))
            status = load_json(job / "status.json")
            return status, (job / "minimized.bin").read_bytes() if (
                job / "minimized.bin"
            ).exists() else None

    def test_minimize_rejects_same_signal_different_location(self):
        status, data = self.minimize(True)
        self.assertEqual(status["state"], "completed")
        self.assertEqual(data, b"A")
        self.assertEqual(status["result"]["signature_strength"], "location_and_frames")

    def test_minimize_refuses_implicit_weak_equivalence(self):
        status, data = self.minimize(False)
        self.assertEqual(status["state"], "failed")
        self.assertIn("location signature unavailable", status["error"])
        self.assertIsNone(data)

    def test_weak_minimization_is_labelled(self):
        status, _ = self.minimize(False, True)
        self.assertEqual(status["result"]["signature_strength"], "signal_only")


if __name__ == "__main__":
    unittest.main()
