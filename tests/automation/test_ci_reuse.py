import io
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts.ci import ci_reuse, ci_scope
from scripts.ci.ci_scope import checks_passed

REPOSITORY = "owner/repo"
HEAD = "a" * 40
OLD_MERGE = "b" * 40
CURRENT_MERGE = "c" * 40
OLD_TREE = "d" * 40
CURRENT_TREE = "e" * 40
NOW = datetime(2026, 11, 23, 12, 0, tzinfo=timezone.utc)


def scope(**changes):
    result = {
        "rust": "true",
        "frontend": "true",
        "linux": "true",
        "apt": "true",
        "uninstall": "true",
        "release_clients": "false",
    }
    result.update(changes)
    return result


def needs(reuse="false"):
    results = {
        name: {"result": "skipped" if reuse == "true" else "success"}
        for name in ("windows", "windows-worker", "linux", "acceptance")
    }
    results.update(
        {
            name: {"result": "skipped"}
            for name in ("windows-client", "linux-client", "linux-desktop-client")
        }
    )
    return {
        "changes": {
            "result": "success",
            "outputs": {
                **scope(),
                "reuse": reuse,
                "reuse_source_run": "101" if reuse == "true" else "",
                "reuse_reason": "test" if reuse == "true" else "",
            },
        },
        "summary": {"result": "success"},
        **results,
    }


def attestation(**changes):
    result = ci_reuse.build_attestation(
        repository=REPOSITORY,
        run_id=101,
        run_attempt=2,
        pr_number=61,
        head_sha=HEAD,
        merge_sha=OLD_MERGE,
        tested_tree=OLD_TREE,
        scope=scope(),
        jobs=needs(),
        created_at=(NOW - timedelta(hours=1)).isoformat(),
    )
    result.update(changes)
    return result


def run(**changes):
    result = {
        "id": 101,
        "run_attempt": 2,
        "event": "pull_request",
        "path": ".github/workflows/ci.yml",
        "repository": {"full_name": REPOSITORY},
        "head_repository": {"full_name": REPOSITORY},
        "status": "completed",
        "conclusion": "success",
        "head_sha": HEAD,
    }
    result.update(changes)
    return result


def api_jobs():
    return [
        {"name": name, "conclusion": "success"}
        for name in (
            "changes",
            "windows",
            "windows-worker",
            "linux",
            "acceptance / acceptance-gate",
            "summary",
        )
    ]


class ReuseTests(unittest.TestCase):
    def equivalent(self, candidate=None, source=None, jobs=None, paths=(), **kwargs):
        arguments = dict(
            repository=REPOSITORY,
            pr_number=61,
            current_merge=CURRENT_MERGE,
            current_tree=CURRENT_TREE,
            current_scope=scope(),
            delta_paths=lambda old, new: list(paths),
            now=NOW,
        )
        arguments.update(kwargs)
        return ci_reuse.equivalent_attestation(
            candidate or attestation(),
            source or run(),
            api_jobs() if jobs is None else jobs,
            **arguments,
        )

    def test_identical_tested_tree_reuses_without_diff(self):
        candidate = attestation(tested_tree=CURRENT_TREE)
        with patch(
            "scripts.ci.ci_reuse.diff_paths", side_effect=AssertionError("no diff expected")
        ):
            self.assertEqual(
                self.equivalent(candidate, delta_paths=ci_reuse.diff_paths),
                "identical tested merge tree",
            )

    def test_documentation_only_delta_reuses_but_code_does_not(self):
        self.assertIn(
            "documentation-only", self.equivalent(paths=["docs/guide.md", ".changes/ci.toml"])
        )
        for paths in (["src/main.rs"], ["docs/guide.md", "Cargo.lock"], [], [".changes/ci.py"]):
            with self.subTest(paths=paths):
                self.assertIsNone(self.equivalent(paths=paths))

    def test_scope_age_identity_and_job_mismatches_fail_closed(self):
        cases = [
            attestation(scope=scope(rust="false")),
            attestation(created_at=(NOW - timedelta(days=8)).isoformat()),
            attestation(created_at=(NOW + timedelta(seconds=1)).isoformat()),
            attestation(repository="owner/other"),
            attestation(pr_number=62),
            attestation(run_attempt=1),
            attestation(head_sha="f" * 40),
            attestation(tested_tree="not-a-tree"),
        ]
        for candidate in cases:
            with self.subTest(candidate=candidate):
                self.assertIsNone(self.equivalent(candidate))
        self.assertIsNone(self.equivalent(source=run(conclusion="failure")))
        self.assertIsNone(self.equivalent(source=run(head_repository={"full_name": "fork/repo"})))
        self.assertIsNone(self.equivalent(jobs=api_jobs()[:-1]))
        reused = attestation()
        reused["jobs"]["windows"]["result"] = "skipped"
        self.assertIsNone(self.equivalent(reused))

    def test_attestation_archive_is_single_bounded_json_file(self):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr(ci_reuse.ATTESTATION_PATH, json.dumps(attestation()))
        self.assertEqual(ci_reuse.read_attestation_archive(buffer.getvalue())["run_id"], "101")
        for name in ("other.json", "ci-result.json/child"):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                archive.writestr(name, "{}")
            with self.subTest(name=name), self.assertRaises(ValueError):
                ci_reuse.read_attestation_archive(buffer.getvalue())
        with self.assertRaises(ValueError):
            ci_reuse.read_attestation_archive(b"x" * (128 * 1024 + 1))

    def test_discovery_binds_artifact_to_successful_attempt(self):
        candidate = attestation(tested_tree=CURRENT_TREE)
        source = run()
        jobs = api_jobs()
        jobs[0].update(started_at="2026-11-23T10:00:00Z", completed_at="2026-11-23T10:01:00Z")
        jobs[-1].update(started_at="2026-11-23T11:00:00Z", completed_at="2026-11-23T11:01:00Z")

        def request(path):
            if "/runs?" in path:
                return {"workflow_runs": [source]}
            if path.endswith("/attempts/2/jobs?per_page=100"):
                return {"jobs": jobs, "total_count": len(jobs)}
            if path.endswith("/artifacts?per_page=100"):
                return {
                    "artifacts": [
                        {
                            "id": 501,
                            "name": ci_reuse.ARTIFACT_NAME,
                            "expired": False,
                            "size_in_bytes": 1000,
                            "created_at": "2026-11-23T11:00:30Z",
                        }
                    ],
                    "total_count": 1,
                }
            raise AssertionError(path)

        result = ci_reuse.discover(
            repository=REPOSITORY,
            pr_number=61,
            head_branch="topic",
            current_run=999,
            current_merge=CURRENT_MERGE,
            current_tree=CURRENT_TREE,
            current_scope=scope(),
            request=request,
            downloader=lambda repository, artifact_id: candidate,
            now=NOW,
        )
        self.assertTrue(result["reuse"])
        self.assertEqual(result["reuse_source_run"], "101")

    def test_gate_requires_verified_reuse_for_selected_skips(self):
        reused = needs(reuse="true")
        self.assertTrue(checks_passed(reused))
        reused["windows"]["result"] = "success"
        self.assertFalse(checks_passed(reused))
        reused = needs(reuse="true")
        reused["changes"]["outputs"]["reuse_source_run"] = ""
        self.assertFalse(checks_passed(reused))
        reused["changes"]["outputs"]["reuse"] = "unknown"
        self.assertFalse(checks_passed(reused))
        actual = needs(reuse="false")
        actual["linux"]["result"] = "skipped"
        self.assertFalse(checks_passed(actual))

    def test_scope_main_emits_verified_reuse_and_honors_force_full(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / "event.json"
            event.write_text(
                json.dumps(
                    {
                        "pull_request": {
                            "number": 61,
                            "base": {"sha": OLD_MERGE},
                            "head": {"sha": HEAD, "ref": "topic"},
                        }
                    }
                ),
                encoding="utf-8",
            )
            output = root / "output.txt"
            environ = {
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_OUTPUT": str(output),
                "GITHUB_REPOSITORY": REPOSITORY,
                "GITHUB_RUN_ID": "999",
                "GITHUB_REF": "refs/pull/61/merge",
                "BUILD_RELEASE_CLIENTS": "false",
                "WINDOWS_WORKER_ACCEPTANCE": "false",
                "FORCE_FULL_CI": "false",
            }
            reuse = {
                "reuse": True,
                "reuse_source_run": "101",
                "reuse_reason": "identical tested merge tree",
            }
            with (
                patch.dict(ci_scope.os.environ, environ, clear=True),
                patch("scripts.ci.ci_scope.changed_paths", return_value=["src/main.rs"]),
                patch("scripts.ci.ci_reuse.discover", return_value=reuse) as discover,
                patch(
                    "scripts.ci.ci_scope.subprocess.check_output",
                    side_effect=[CURRENT_MERGE + "\n", CURRENT_TREE + "\n"],
                ),
            ):
                ci_scope.main()
            self.assertIn("reuse=true\n", output.read_text(encoding="utf-8"))
            self.assertEqual(discover.call_args.kwargs["current_tree"], CURRENT_TREE)
            environ["FORCE_FULL_CI"] = "true"
            output.write_text("", encoding="utf-8")
            with (
                patch.dict(ci_scope.os.environ, environ, clear=True),
                patch("scripts.ci.ci_scope.changed_paths", return_value=["src/main.rs"]),
                patch("scripts.ci.ci_reuse.discover") as discover,
            ):
                ci_scope.main()
            discover.assert_not_called()
            self.assertIn("reuse=false\n", output.read_text(encoding="utf-8"))

    def test_workflow_reuse_is_gated_and_attestation_is_optional(self):
        workflow = Path(".github/workflows/ci.yml").read_text(encoding="utf-8")
        changes = workflow[workflow.index("  changes:") : workflow.index("  windows:")]
        for name in ("reuse", "reuse_source_run", "reuse_reason"):
            self.assertIn(f"{name}: ${{{{ steps.scope.outputs.{name} }}}}", changes)
        self.assertEqual(workflow.count("needs.changes.outputs.reuse != 'true'"), 7)
        self.assertIn("needs.changes.outputs.reuse == 'false'", workflow)
        self.assertIn("name: ci-test-attestation-v1", workflow)
        self.assertIn("FORCE_FULL_CI:", workflow)
        summary = workflow[workflow.index("  summary:") : workflow.index("  release-request:")]
        self.assertEqual(summary.count("continue-on-error: true"), 2)

    def test_write_attestation_binds_checkout_and_refuses_reused_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / "event.json"
            event.write_text(
                json.dumps({"pull_request": {"number": 61, "head": {"sha": HEAD}}}),
                encoding="utf-8",
            )
            environ = {
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_REPOSITORY": REPOSITORY,
                "GITHUB_RUN_ID": "101",
                "GITHUB_RUN_ATTEMPT": "2",
                "GITHUB_SHA": CURRENT_MERGE,
            }
            current = needs()
            del current["summary"]  # The real summary job's needs context excludes itself.
            current["changes"]["outputs"]["reuse"] = "false"
            with patch(
                "scripts.ci.ci_reuse.subprocess.check_output",
                side_effect=[CURRENT_MERGE + "\n", CURRENT_TREE + "\n"],
            ):
                result = ci_reuse.write_attestation(
                    root / "attestation.json", environ=environ, needs=current
                )
            self.assertEqual(result["merge_sha"], CURRENT_MERGE)
            self.assertEqual(result["tested_tree"], CURRENT_TREE)
            current["changes"]["outputs"]["reuse"] = "true"
            with self.assertRaises(ValueError):
                ci_reuse.write_attestation(root / "reused.json", environ=environ, needs=current)


if __name__ == "__main__":
    unittest.main()
