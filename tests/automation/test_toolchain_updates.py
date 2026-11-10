import base64
import copy
import hashlib
from pathlib import Path
import unittest

import scripts.toolchain.check_toolchain_updates as updates


class ToolchainTests(unittest.TestCase):
    def setUp(self):
        self.current = {"version": "3.3.3", "sha256": "a" * 64, "file": "pwninit", "url": "old"}
        self.release = {"tag_name": "3.3.4", "assets": [{"name": "pwninit", "digest": "sha256:" + "b" * 64,
            "browser_download_url": "https://github.com/io12/pwninit/releases/download/3.3.4/pwninit"}]}

    def inspect(self):
        return updates.inspect("pwninit", self.current, lambda path: self.release)

    def test_proposal_keeps_local_filename_and_does_not_modify_lock(self):
        before = copy.deepcopy(self.current)
        row = self.inspect()
        self.assertEqual(row["status"], "update_available")
        self.assertEqual(row["candidate"]["file"], "pwninit")
        self.assertEqual(self.current, before)

    def test_missing_checksum_is_not_a_verified_update(self):
        self.release["assets"][0]["digest"] = None
        self.assertEqual(self.inspect()["status"], "checksum_review_required")
        self.assertIsNone(self.inspect()["candidate"])

    def test_same_version_different_hash_is_reported(self):
        self.current["version"] = "3.3.4"
        self.assertEqual(self.inspect()["status"], "checksum_changed")

    def test_no_downgrade_and_numeric_version_order(self):
        self.current["version"] = "3.10.0"
        self.assertEqual(self.inspect()["status"], "upstream_older")

    def test_ambiguous_asset_and_external_url_are_rejected(self):
        self.release["assets"] *= 2
        with self.assertRaisesRegex(ValueError, "exactly one"):
            self.inspect()
        self.release["assets"] = self.release["assets"][:1]
        self.release["assets"][0]["browser_download_url"] = "https://example.com/pwninit"
        with self.assertRaisesRegex(ValueError, "URL"):
            self.inspect()

    def test_gef_pins_source_commit_and_marks_review(self):
        content = b"# upstream source\n"
        blob = hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest()
        replies = iter([{"tag_name": "2026.01"}, {"sha": "c" * 40},
                        {"encoding": "base64", "type": "file", "sha": blob, "size": len(content),
                         "content": base64.b64encode(content).decode()}])
        row = updates.inspect("gef", {**self.current, "version": "2024.06"}, lambda path: next(replies))
        self.assertEqual(row["status"], "source_update_review")
        self.assertIn("c" * 40, row["candidate"]["url"])
        self.assertEqual(row["candidate"]["sha256"], hashlib.sha256(content).hexdigest())

    def test_one_network_failure_does_not_erase_other_findings(self):
        manifest = {"schema_version": 1, "platform": "linux/amd64", **{k: self.current for k in updates.SOURCES}}
        def unavailable(path):
            raise RuntimeError("HTTP 503")
        report = updates.check(manifest, unavailable)
        self.assertFalse(report["passed"])
        self.assertEqual(len(report["tools"]), 5)
        self.assertTrue(all(r["status"] == "error" for r in report["tools"]))


if __name__ == "__main__":
    unittest.main()
