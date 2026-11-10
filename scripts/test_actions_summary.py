import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("actions_summary", Path(__file__).with_name("actions-summary.py"))
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


class SummaryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        self.context = {"repository": "owner/repo", "run_id": "1", "run_url": "https://github.com/owner/repo/actions/runs/1",
                        "ref": "refs/heads/topic|<script>", "sha": "abc", "event": "workflow_dispatch", "ref_name": "topic"}
        self.needs = {n: {"result": "success"} for n in ("metadata", "checks", "windows-client", "linux-client", "image", "acceptance", "package")}
        self.needs["publish"] = {"result": "skipped"}

    def tearDown(self):
        self.temporary.cleanup()

    def test_manual_release_is_successful_without_claiming_publication(self):
        result = summary.render("release", self.needs, self.context, self.directory)
        self.assertIn("通过 / Passed", result)
        self.assertIn("不创建正式 Release", result)
        self.assertNotIn("已发布的不可变", result)
        self.assertIn("topic&#124;&lt;script&gt;", result)

    def test_failure_and_missing_reports_do_not_look_successful(self):
        self.needs["acceptance"]["result"] = "failure"
        self.needs["package"]["result"] = "skipped"
        result = summary.render("release", self.needs, self.context, self.directory)
        self.assertIn("Incomplete or failed", result)
        self.assertIn("尚无容器验收报告", result)

    def test_tag_requires_successful_publish(self):
        self.context["event"] = "push"
        result = summary.render("release", self.needs, self.context, self.directory)
        self.assertIn("Incomplete or failed", result)

    def test_reused_clients_only_count_when_source_is_present(self):
        self.needs["windows-client"]["result"] = "skipped"
        self.assertIn("Incomplete or failed", summary.render("release", self.needs, self.context, self.directory))
        self.needs["metadata"]["outputs"] = dict(windows_run="123")
        result = summary.render("release", self.needs, self.context, self.directory)
        self.assertIn("通过 / Passed", result)
        self.assertIn("/actions/runs/123", result)

    def test_package_details_and_artifact_link_come_from_real_metadata(self):
        folder = self.directory / "release"
        folder.mkdir()
        (folder / "release.json").write_text(json.dumps({"version": "0.1.0", "image": "owner/tool@sha256:abc",
            "tool_platform": "linux/amd64", "image_size_bytes": 1024 ** 3}), encoding="utf-8")
        (folder / "archives.json").write_text(json.dumps([dict(name="client.zip", size_bytes=4 * 1024 ** 2)]))
        result = summary.render("release", self.needs, self.context, self.directory, artifacts=[{
            "id": 42, "name": "online-release", "size_in_bytes": 100, "expired": False}])
        self.assertIn("owner/tool@sha256:abc", result)
        self.assertIn("1.00 GiB", result)
        self.assertIn("client.zip: 4.00 MiB", result)
        self.assertIn("/artifacts/42", result)

    def test_reusable_job_duration_and_missing_timestamps(self):
        jobs = [{"name": "checks / windows", "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:02:05Z"}]
        self.assertEqual(summary.duration("windows", jobs), "2m 5s")
        self.assertEqual(summary.duration("checks", jobs), "2m 5s")
        jobs[0]["completed_at"] = None
        self.assertEqual(summary.duration("windows", jobs), "—")

    def test_ci_scope_cache_and_step_durations(self):
        needs = {"changes": {"result": "success", "outputs": dict(rust="false", frontend="false", linux="false")},
                 "windows": {"result": "success", "outputs": {"rust_cache_hit": "true"}},
                 "linux": {"result": "skipped"}}
        jobs = [{"name": "windows", "steps": [{"name": "Rust tests", "conclusion": "success",
                 "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:01:05Z"}]}]
        result = summary.render("ci", needs, self.context, self.directory, jobs=jobs)
        self.assertIn("通过 / Passed", result)
        self.assertIn("Rust cache: 精确命中", result)
        self.assertIn("Rust tests | success | 1m 5s", result)
        needs["changes"]["outputs"]["linux"] = "true"
        result = summary.render("ci", needs, self.context, self.directory)
        self.assertIn("Incomplete or failed", result)


if __name__ == "__main__":
    unittest.main()
