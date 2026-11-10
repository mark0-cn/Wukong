import importlib.util
from pathlib import Path
import subprocess
import tempfile
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


class ImmutableReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        self.asset = self.directory / "release.json"
        self.asset.write_bytes(b"accepted bytes")
        self.release = {"draft": False, "immutable": True, "assets": [{
            "name": self.asset.name, "state": "uploaded", "size": self.asset.stat().st_size,
            "digest": "sha256:" + publish.sha256(self.asset)}]}

    def tearDown(self):
        self.temporary.cleanup()

    def test_exact_uploaded_assets_can_be_published_and_retried(self):
        publish.verify_assets(self.directory, self.release)
        publish.verify_assets(self.directory, self.release, require_immutable=True)

    def test_draft_lookup_uses_release_id_not_published_tag_endpoint(self):
        with patch.object(publish, "releases", return_value=[[{"id": 42, "tag_name": "v0.1.0", "draft": True}]]), patch.object(publish, "run", return_value='{"draft":true}') as run:
            self.assertTrue(publish.release_by_tag("owner/repo", "v0.1.0")["draft"])
            run.assert_called_once_with(["gh", "api", "repos/owner/repo/releases/42"])

    def test_stale_draft_asset_is_rejected_before_locking(self):
        self.release["assets"].append({"name": "stale.zip"})
        with self.assertRaisesRegex(ValueError, "accepted set"):
            publish.verify_assets(self.directory, self.release)

    def test_changed_uploaded_bytes_are_rejected(self):
        self.release["assets"][0]["digest"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            publish.verify_assets(self.directory, self.release)

    def test_mutable_release_cannot_advance_latest(self):
        self.release["immutable"] = False
        with self.assertRaisesRegex(ValueError, "not immutable"):
            publish.verify_assets(self.directory, self.release, require_immutable=True)


if __name__ == "__main__":
    unittest.main()
