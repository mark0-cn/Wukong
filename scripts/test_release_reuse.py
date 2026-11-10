import unittest
from release_reuse import discover, successful_artifact, trusted_run


class ReuseTests(unittest.TestCase):
    def setUp(self):
        self.run = dict(id=1, run_attempt=2, event="push", head_branch="master", head_sha="a" * 40,
                        path=".github/workflows/ci.yml", status="completed", conclusion="success",
                        repository=dict(full_name="o/r"), head_repository=dict(full_name="o/r"))
        self.jobs = [dict(name=n, conclusion="success") for n in ("changes", "windows", "linux", "summary")]
        self.jobs.append(dict(name="windows-client / build", conclusion="success",
                             started_at="2026-01-02T01:00:00Z", completed_at="2026-01-02T01:02:00Z"))
        self.artifact = dict(name="windows-client-v1", expired=False, size_in_bytes=100,
                             created_at="2026-01-02T01:01:00Z")

    def test_wrong_source_event_workflow_and_repo_never_reused(self):
        self.assertTrue(trusted_run(self.run, "o/r", "a" * 40))
        for change in (dict(head_sha="b" * 40), dict(event="pull_request"), dict(head_branch="topic"),
                       dict(path="other.yml"), dict(status="in_progress"), dict(head_repository=dict(full_name="fork/r"))):
            self.assertFalse(trusted_run(dict(self.run, **change), "o/r", "a" * 40))

    def test_expired_wrong_and_previous_attempt_artifacts_are_rejected(self):
        self.assertTrue(successful_artifact("windows", self.jobs, [self.artifact]))
        for change in (dict(expired=True), dict(name="windows-client"), dict(size_in_bytes=0),
                       dict(created_at="2026-01-01T00:00:00Z")):
            self.assertFalse(successful_artifact("windows", self.jobs, [dict(self.artifact, **change)]))
        self.jobs[-1]["conclusion"] = "failure"
        self.assertFalse(successful_artifact("windows", self.jobs, [self.artifact]))

    def test_exact_ci_reuse_and_partial_build_fallback(self):
        paths = []
        def request(path):
            paths.append(path)
            if "/attempts/" in path:
                return dict(jobs=self.jobs, total_count=len(self.jobs))
            if "/artifacts?" in path:
                return dict(artifacts=[self.artifact])
            return dict(workflow_runs=[self.run])
        self.assertEqual(discover("o/r", "a" * 40, 99, request), dict(ci_run="1", windows_run="1", linux_run=""))
        self.assertTrue(any("/attempts/2/jobs" in path for path in paths))
        self.run["conclusion"] = "failure"
        self.assertEqual(discover("o/r", "a" * 40, 99, request), dict(ci_run="", windows_run="1", linux_run=""))
        self.run["run_attempt"] = 1
        self.assertEqual(discover("o/r", "a" * 40, 1, request), dict(ci_run="", windows_run="", linux_run=""))

    def test_whole_run_retry_reads_only_previous_completed_attempt(self):
        previous = dict(self.run, run_attempt=1, path=".github/workflows/release.yml", event="workflow_dispatch")
        current = dict(previous, run_attempt=2, status="in_progress", conclusion=None)
        def request(path):
            if path.endswith("/attempts/1"):
                return previous
            if "/attempts/1/jobs" in path:
                return dict(jobs=self.jobs, total_count=len(self.jobs))
            if "/artifacts?" in path:
                return dict(artifacts=[self.artifact])
            if "/attempts/2" in path:
                self.fail("Must not consume jobs from running attempt")
            return dict(workflow_runs=[current])
        self.assertEqual(discover("o/r", "a" * 40, 1, request), dict(ci_run="", windows_run="1", linux_run=""))
