import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("publish_release", Path(__file__).with_name("publish-release.py"))
publish = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publish)


class PromotionTests(unittest.TestCase):
    def test_published_version_cannot_be_replaced(self):
        with patch.object(publish, "image_digest", return_value="sha256:" + "a" * 64), patch.object(publish, "run") as run:
            with self.assertRaisesRegex(ValueError, "overwrite"):
                publish.promote("hlmark0/re-agent-workbench", "sha256:" + "b" * 64, "0.1.0")
            run.assert_not_called()

    def test_retry_of_identical_promotion_is_noop(self):
        digest = "sha256:" + "a" * 64
        with patch.object(publish, "image_digest", return_value=digest), patch.object(publish, "run") as run:
            publish.promote("hlmark0/re-agent-workbench", digest, "0.1.0")
            run.assert_not_called()

    def test_changed_index_after_promotion_is_rejected(self):
        with patch.object(publish, "image_digest", side_effect=[None, "sha256:" + "c" * 64]), patch.object(publish, "run"):
            with self.assertRaisesRegex(ValueError, "changed"):
                publish.promote("hlmark0/re-agent-workbench", "sha256:" + "a" * 64, "0.1.0")

    def test_registry_auth_failure_is_not_treated_as_missing(self):
        with patch.object(publish.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "unauthorized: authentication required")):
            with self.assertRaises(RuntimeError):
                publish.image_digest("example", missing_ok=True)

    def test_missing_candidate_can_be_built(self):
        with patch.object(publish.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "manifest unknown: not found")):
            self.assertIsNone(publish.image_digest("example", missing_ok=True))


if __name__ == "__main__":
    unittest.main()
