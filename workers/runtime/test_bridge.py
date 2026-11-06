"""Portable runtime transport tests; all executed commands are trusted fixtures."""
import json
import os
from pathlib import Path
import sys
import signal
import tempfile
import time
import unittest
from integration_test import Client


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.client = Client(self.root, trusted_local=True, environment={**os.environ, "OPENAI_API_KEY": "fixture-secret", "ANTHROPIC_API_KEY": "fixture-secret"})

    def tearDown(self):
        try:
            self.client.call("worker.shutdown")
        finally:
            self.client.close()
            # Windows may still have the final shutdown frame/log open briefly.
            for _ in range(30):
                try:
                    self.temporary.cleanup()
                    break
                except PermissionError:
                    time.sleep(0.05)

    def test_reconnect_and_request_identity(self):
        params = {"argv": [sys.executable, "-c", "import time;time.sleep(0.5);print('durable')"], "request_id": "durable-1", "timeout_secs": 4}
        job = self.client.call("job.start", **params)
        self.client.close()
        self.client = Client(self.root, trusted_local=True)
        duplicate = self.client.call("job.start", **params)
        self.assertEqual(job["job_id"], duplicate["job_id"])
        result = self.client.finish(job)
        self.assertEqual(result["state"], "completed")
        self.assertIn("durable", result["result"]["stdout"])
        self.assertEqual(len(self.client.call("job.list")), 1)

    def test_output_cap_and_timeout(self):
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "print('a'*200000)"], output_limit=1024))
        self.assertTrue(result["result"]["output_truncated"])
        self.assertLessEqual(len(result["result"]["stdout"]), 1024)
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "import time;time.sleep(60)"], timeout_secs=1))
        self.assertEqual(result["state"], "timed_out")

    def test_supervisor_survives_owner_daemon_restart(self):
        job = self.client.call("job.start", argv=[sys.executable, "-c", "import time;time.sleep(1);print('owner-recovered')"], request_id="owner-recovery")
        daemon_pid = self.client.call("ping")["daemon_pid"]
        self.client.close()
        os.kill(daemon_pid, signal.SIGTERM)
        self.client = Client(self.root, trusted_local=True)
        result = self.client.finish(job)
        self.assertEqual(result["state"], "completed")
        self.assertIn("owner-recovered", result["result"]["stdout"])

    def test_cancel_kills_descendants(self):
        marker = self.root / "child-survived.txt"
        child = "import time,pathlib;time.sleep(2);pathlib.Path(" + repr(str(marker)) + ").write_text('alive')"
        command = "import subprocess,sys,time;subprocess.Popen([sys.executable,'-c'," + repr(child) + "]);time.sleep(60)"
        job = self.client.call("job.start", argv=[sys.executable, "-c", command], timeout_secs=10)
        time.sleep(0.3)
        self.client.call("job.cancel", job_id=job["job_id"], request_id="cancel-1")
        self.assertEqual(self.client.finish(job)["state"], "cancelled")
        time.sleep(2.1)
        self.assertFalse(marker.exists(), "cancelled command left a descendant running")

    def test_worker_receives_no_provider_credentials(self):
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "import os;print(','.join(k for k in os.environ if 'API_KEY' in k or 'TOKEN' in k))"]))
        self.assertEqual(result["result"]["stdout"].strip(), "")
        self.assertEqual(self.client.call("doctor")["protocol"], 1)

    def test_artifact_confinement(self):
        self.client.put("example.txt", b"example")
        # Client.call asserts worker errors, which is intentional in these cases.
        with self.assertRaises(AssertionError):
            self.client.call("file.get", path="../outside")
        with self.assertRaises(AssertionError):
            self.client.call("file.get", path="endpoint.json")
        with self.assertRaises(AssertionError):
            self.client.call("file.put", path="../../outside", data_base64="eA==")
        with self.assertRaises(AssertionError):
            self.client.call("job.start", argv=[sys.executable, "-c", "pass"], env={"OPENAI_API_KEY": "rejected"})


if __name__ == "__main__":
    unittest.main()

