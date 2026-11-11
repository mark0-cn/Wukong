import copy
import unittest
from scripts.release.recover_publish import select


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.run = dict(status="completed", event="push", path=".github/workflows/release.yml", head_sha="a" * 40,
                        head_branch="v0.2.0", repository=dict(full_name="o/r"), head_repository=dict(full_name="o/r"))
        self.jobs = [dict(name=name, conclusion="success", started_at="2026-01-01T00:00:00Z", completed_at="2026-01-01T00:02:00Z")
                     for name in ("metadata", "checks", "image", "acceptance", "windows-installer / build", "linux-installer / build", "package")]
        self.artifacts = [dict(id=42, name="online-release", expired=False, size_in_bytes=10, created_at="2026-01-01T00:01:00Z")]

    def test_skipped_publisher_does_not_invalidate_accepted_package(self):
        self.jobs.append(dict(name="publish", conclusion="skipped"))
        self.assertEqual(select(self.run, self.jobs, self.artifacts, "o/r"), dict(tag="v0.2.0", sha="a" * 40, artifact_id="42"))

    def test_fork_manual_wrong_workflow_and_pending_runs_rejected(self):
        for change in (dict(event="workflow_dispatch"), dict(event="pull_request"), dict(path="other.yml"),
                       dict(head_repository=dict(full_name="fork/r")), dict(status="in_progress"), dict(head_branch="v01.0.0")):
            with self.subTest(change=change), self.assertRaises(ValueError):
                select(dict(self.run, **change), self.jobs, self.artifacts, "o/r")

    def test_each_gate_and_artifact_age_are_required(self):
        for index in range(len(self.jobs)):
            broken = copy.deepcopy(self.jobs)
            broken[index]["conclusion"] = "skipped"
            with self.assertRaises(ValueError):
                select(self.run, broken, self.artifacts, "o/r")
        for change in (dict(expired=True), dict(created_at="2025-01-01T00:01:00Z"), dict(name="other")):
            with self.assertRaises(ValueError):
                select(self.run, self.jobs, [dict(self.artifacts[0], **change)], "o/r")
