import copy
import unittest
from ci_scope import checks_passed, select_scope


class ScopeTests(unittest.TestCase):
    def test_master_manual_and_release_always_run_full_checks(self):
        for event in ("push", "workflow_dispatch", "workflow_call"):
            scope = select_scope(["docs/releases.md"], event)
            self.assertTrue(all(scope[name] for name in ("rust", "frontend", "linux")))
        self.assertTrue(select_scope(["ui/package.json"], "pull_request", "codex/release-v0.2.0")["rust"])

    def test_known_paths_select_only_relevant_expensive_checks(self):
        for paths, expected in [(["README.md", "docs/releases.md"], (False, False, False)),
                                (["ui/src/App.tsx"], (False, True, False)),
                                (["workers/runtime/requirements.txt"], (False, False, True)),
                                (["fixtures/analysis/test_analysis.py", "ui/package.json"], (False, True, True))]:
            with self.subTest(paths=paths):
                scope = select_scope(paths, "pull_request")
                self.assertEqual(tuple(scope[name] for name in ("rust", "frontend", "linux")), expected)

    def test_shared_unknown_and_renamed_paths_fail_closed(self):
        for paths in ([], ["Cargo.lock"], ["scripts/release.py"], ["src/protocol.rs"],
                      [".github/workflows/ci.yml"], ["unknown.txt"], ["docs/example.py"],
                      ["src/old.rs", "docs/new.md"]):
            with self.subTest(paths=paths):
                scope = select_scope(paths, "pull_request")
                self.assertTrue(all(scope[name] for name in ("rust", "frontend", "linux")))

    def test_gate_accepts_only_expected_results(self):
        for linux in ("true", "false"):
            needs = {"changes": {"result": "success", "outputs": dict(rust="false", frontend="false", linux=linux)},
                     "windows": {"result": "success"},
                     "linux": {"result": "success" if linux == "true" else "skipped"}}
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
