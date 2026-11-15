import copy
import unittest
from scripts.ci.ci_scope import checks_passed, select_scope, prebuild_clients, changed_paths
from unittest.mock import patch
from scripts.ci.acceptance_gate import checks_passed as acceptance_passed


class ScopeTests(unittest.TestCase):
    def test_explicit_worker_acceptance_builds_all_required_components_even_for_documentation(self):
        scope = select_scope(["README.md"], "pull_request", force_full=True)
        self.assertTrue(all(scope[name] for name in ("rust", "frontend", "linux")))

    def test_release_fragments_preserve_frontend_scope_but_unknown_note_files_do_not(self):
        selected = select_scope(
            ["ui/src/features/maintenance/Updates.tsx", ".changes/release-notes.toml"],
            "pull_request",
        )
        self.assertTrue(selected["frontend"])
        self.assertFalse(selected["rust"])
        self.assertFalse(selected["linux"])
        self.assertTrue(select_scope([".changes/executable.py"], "pull_request")["rust"])

    def test_clients_only_for_master_version_increase_or_explicit_prewarm(self):
        self.assertTrue(prebuild_clients("push", "refs/heads/master", False, "0.1.2", "0.1.3"))
        self.assertTrue(
            prebuild_clients("workflow_dispatch", "refs/heads/master", True, "0.1.2", "0.1.2")
        )
        for event, ref, requested, old, new in [
            ("push", "refs/heads/master", False, "0.1.2", "0.1.2"),
            ("push", "refs/heads/master", False, "0.2.0", "0.1.2"),
            ("push", "refs/tags/v0.1.3", True, "0.1.2", "0.1.3"),
            ("pull_request", "refs/heads/master", True, "0.1.2", "0.1.3"),
            ("workflow_dispatch", "refs/heads/topic", True, "0.1.2", "0.1.3"),
            ("workflow_dispatch", "refs/heads/master", False, "0.1.2", "0.1.3"),
        ]:
            self.assertFalse(prebuild_clients(event, ref, requested, old, new))

    def test_required_prebuild_failure_blocks_ci(self):
        needs = {
            "changes": {
                "result": "success",
                "outputs": dict(
                    rust="true",
                    frontend="true",
                    linux="true",
                    apt="false",
                    uninstall="false",
                    release_clients="true",
                ),
            },
            **{
                name: {"result": "success"}
                for name in (
                    "windows",
                    "linux",
                    "windows-client",
                    "linux-client",
                    "linux-desktop-client",
                )
            },
        }
        needs["acceptance"] = {"result": "skipped"}
        self.assertTrue(checks_passed(needs))
        needs["linux-client"]["result"] = "skipped"
        self.assertFalse(checks_passed(needs))
        needs["changes"]["outputs"]["release_clients"] = "false"
        needs["windows-client"]["result"] = "skipped"
        needs["linux-desktop-client"]["result"] = "skipped"
        self.assertTrue(checks_passed(needs))

    def test_manual_and_release_always_run_full_checks(self):
        for event in ("workflow_dispatch", "workflow_call"):
            scope = select_scope(["docs/development/releases.md"], event)
            self.assertTrue(all(scope[name] for name in ("rust", "frontend", "linux")))
        self.assertTrue(
            select_scope(["ui/package.json"], "pull_request", "codex/release-v0.2.0")["rust"]
        )

    def test_known_paths_select_only_relevant_expensive_checks(self):
        for paths, expected in [
            (
                [
                    "README.md",
                    "README.en.md",
                    "README.zh-CN.md",
                    "docs/translations.json",
                    "docs/development/releases.zh-CN.md",
                ],
                (False, False, False),
            ),
            (["ui/src/app/App.tsx"], (False, True, False)),
            (["workers/runtime/requirements.txt"], (False, False, True)),
            (["workers/tests/unit/test_profile.py"], (False, False, True)),
            (["evals/analysis/score.py", "fixtures/src/benign.c"], (False, False, True)),
            (["evals/tests/test_analysis.py", "ui/package.json"], (False, True, True)),
        ]:
            with self.subTest(paths=paths):
                scope = select_scope(paths, "pull_request")
                self.assertEqual(
                    tuple(scope[name] for name in ("rust", "frontend", "linux")), expected
                )

    def test_shared_unknown_and_renamed_paths_fail_closed(self):
        for paths in (
            [],
            ["Cargo.lock"],
            ["scripts/release/cli.py"],
            ["src/protocol.rs"],
            [".github/workflows/ci.yml"],
            ["unknown.txt"],
            ["docs/example.py"],
            ["src/old.rs", "docs/new.md"],
            ["locales/en-US/common.ftl"],
        ):
            with self.subTest(paths=paths):
                scope = select_scope(paths, "pull_request")
                self.assertTrue(all(scope[name] for name in ("rust", "frontend", "linux")))

    def test_gate_accepts_only_expected_results(self):
        for linux in ("true", "false"):
            needs = {
                "changes": {
                    "result": "success",
                    "outputs": dict(
                        rust="false",
                        frontend="false",
                        linux=linux,
                        apt="false",
                        uninstall="false",
                        release_clients="false",
                    ),
                },
                "windows": {"result": "success" if linux == "true" else "skipped"},
                **{
                    name: {"result": "skipped"}
                    for name in (
                        "acceptance",
                        "windows-client",
                        "linux-client",
                        "linux-desktop-client",
                    )
                },
                "linux": {"result": "success" if linux == "true" else "skipped"},
            }
            self.assertTrue(checks_passed(needs))
            for job in ("changes", "windows", "linux"):
                for result in ("failure", "cancelled", "skipped", "success"):
                    if result == needs[job]["result"]:
                        continue
                    broken = copy.deepcopy(needs)
                    broken[job]["result"] = result
                    self.assertFalse(checks_passed(broken), (job, result))
            broken = copy.deepcopy(needs)
            del broken["changes"]["outputs"]["rust"]
            self.assertFalse(checks_passed(broken))
        self.assertFalse(checks_passed({}))

    def test_docs_are_lightweight_on_pr_and_master_but_runtime_prompts_are_not(self):
        for event in ("pull_request", "push"):
            scope = select_scope(["AGENTS.md", "AGENTS.zh-CN.md", "docs/translations.json"], event)
            self.assertFalse(
                any(scope[k] for k in ("rust", "frontend", "linux", "apt", "uninstall"))
            )
            self.assertTrue(select_scope(["prompts/root.md"], event)["rust"])

    def test_scope_does_not_lose_acceptance_paths_after_a_shared_path(self):
        for paths, apt, uninstall in [
            (["Cargo.lock", "scripts/ci/install_apt.py"], True, False),
            (["Cargo.toml", "src/application/maintenance/cleanup/executor.rs"], False, True),
            ([".github/workflows/ci.yml"], True, True),
            (["ui/src/features/maintenance/CleanupReview.tsx"], False, True),
            (["src/interfaces/cli/cleanup.rs"], False, True),
            (["src/interfaces/desktop/cleanup.rs"], False, True),
            (["src/interfaces/desktop/cleanup_review.rs"], False, True),
            (["src/adapters/files/owned.rs"], False, True),
            (["src/adapters/files/paths.rs"], False, True),
            (["src/application/maintenance/updates/recovery.rs"], False, True),
            (["ui/src/shared/api/commands.ts"], False, True),
        ]:
            for event in ("pull_request", "push"):
                scope = select_scope(paths, event)
                self.assertEqual((scope["apt"], scope["uninstall"]), (apt, uninstall))

    def test_push_scope_uses_entire_before_after_range_and_pr_uses_merge_base(self):
        with patch(
            "scripts.ci.ci_scope.subprocess.check_output", return_value=b"src/old.rs\0docs/new.md\0"
        ) as run:
            self.assertEqual(changed_paths("a" * 40, "b" * 40), ["src/old.rs", "docs/new.md"])
            self.assertEqual(run.call_args.args[0][-1], "a" * 40 + ".." + "b" * 40)
            changed_paths("a" * 40, "b" * 40, pull_request=True)
            self.assertEqual(run.call_args.args[0][-1], "a" * 40 + "..." + "b" * 40)
            with self.assertRaises(ValueError):
                changed_paths("0" * 40, "b" * 40)

    def test_every_selected_acceptance_failure_blocks_both_gates(self):
        needs = {
            "changes": {
                "result": "success",
                "outputs": dict(
                    rust="true",
                    frontend="true",
                    linux="true",
                    apt="true",
                    uninstall="true",
                    release_clients="false",
                ),
            },
            **{name: {"result": "success"} for name in ("windows", "linux", "acceptance")},
            **{
                name: {"result": "skipped"}
                for name in ("windows-client", "linux-client", "linux-desktop-client")
            },
        }
        self.assertTrue(checks_passed(needs))
        for outcome in ("failure", "cancelled", "skipped", ""):
            needs["acceptance"]["result"] = outcome
            self.assertFalse(checks_passed(needs))
        jobs = {name: {"result": "success"} for name in ("apt", "uninstall", "windows-worker")}
        self.assertTrue(acceptance_passed(jobs, "all"))
        for job in jobs:
            for outcome in ("failure", "cancelled", "skipped", ""):
                broken = copy.deepcopy(jobs)
                broken[job]["result"] = outcome
                self.assertFalse(acceptance_passed(broken, "all"))
        self.assertFalse(acceptance_passed(jobs, "unknown"))
        jobs["windows-worker"]["result"] = "skipped"
        self.assertTrue(acceptance_passed(jobs, apt=True, uninstall=True))
        self.assertFalse(acceptance_passed(jobs))
