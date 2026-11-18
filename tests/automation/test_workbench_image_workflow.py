"""Guard the manual workbench-image workflow's trigger, repository and tag rules.

The workflow publishes the separate `<namespace>/wukong-workbench` repository
only: it must never push the official release image, move a `latest` tag or
require administrator-only gating.
"""

from pathlib import Path
import re
import unittest

WORKFLOW = Path(".github/workflows/workbench-image.yml")
RELEASE = Path(".github/workflows/release.yml")
OFFICIAL_REPOSITORY = re.compile(r"\$\{\{\s*vars\.DOCKERHUB_NAMESPACE\s*\}\}/wukong(?!-)")


class WorkbenchImageWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.release = RELEASE.read_text(encoding="utf-8")

    def triggers(self):
        lines = self.workflow.splitlines()
        result = []
        for line in lines[lines.index("on:") + 1 :]:
            if line.strip() and not line.startswith(" "):
                break
            if re.match(r"^  [A-Za-z0-9_-]+:", line):
                result.append(line.strip())
        return result

    def dispatch_block(self):
        start = self.workflow.index("workflow_dispatch:")
        end = self.workflow.index("permissions:")
        return self.workflow[start:end]

    def test_workflow_exists_and_is_manual_only(self):
        self.assertTrue(WORKFLOW.is_file())
        self.assertEqual(self.triggers(), ["workflow_dispatch:"])
        for automatic in ("push:", "pull_request:", "schedule:", "workflow_call:", "workflow_run:"):
            self.assertNotIn(automatic, self.workflow.split("permissions:")[0])

    def test_requires_a_version_and_accepts_an_optional_ref(self):
        dispatch = self.dispatch_block()
        version = re.search(r"^      version:\n((?:        .*\n)+)", dispatch, re.M).group(1)
        self.assertIn("required: true", version)
        ref = re.search(r"^      ref:\n((?:        .*\n)+)", dispatch, re.M).group(1)
        self.assertIn("required: false", ref)

    def test_pushes_only_the_separate_workbench_repository(self):
        self.assertIn("wukong-workbench:local-", self.workflow)
        # The official release image is <namespace>/wukong; the executable
        # workflow must never construct that path (with or without a tag).
        code = "\n".join(
            line for line in self.workflow.splitlines() if not line.lstrip().startswith("#")
        )
        self.assertIsNone(OFFICIAL_REPOSITORY.search(code))
        self.assertIsNone(re.search(r"/wukong(?![-\w])", code))
        self.assertNotIn("hlmark0/wukong", code)
        self.assertIn('docker push "$IMAGE"', self.workflow)

    def test_publishes_an_immutable_version_tag_without_latest(self):
        self.assertIn("local-$VERSION", self.workflow)
        self.assertNotRegex(self.workflow, r":latest\b")
        self.assertIn("imagetools inspect", self.workflow)
        self.assertIn("version tags are immutable", self.workflow)

    def test_reuses_the_release_image_docker_hub_credentials(self):
        for credential in (
            "vars.DOCKERHUB_USERNAME",
            "vars.DOCKERHUB_NAMESPACE",
            "secrets.DOCKERHUB_TOKEN",
        ):
            self.assertIn(credential, self.workflow)
            self.assertIn(credential, self.release)

    def test_has_no_administrator_only_gating(self):
        self.assertNotIn("environment:", self.workflow)
        for marker in ("github.actor ==", "triggering_actor ==", "author_association", "ADMIN"):
            self.assertNotIn(marker, self.workflow)
        self.assertNotIn("permissions: write", self.workflow)
        self.assertIn("permissions:\n  contents: read", self.workflow)

    def test_builds_and_verifies_the_workbench_target_before_publishing(self):
        self.assertIn("python -m scripts.release.cli check", self.workflow)
        self.assertIn("python -m scripts.toolchain.fetch_worker_deps", self.workflow)
        self.assertIn("docker buildx bake", self.workflow)
        self.assertIn("--set workbench.tags", self.workflow)
        self.assertIn("linux/amd64", self.workflow)
        self.assertIn("docker image inspect", self.workflow)
        self.assertIn("docker run --rm", self.workflow)

    def test_run_summary_records_digest_size_and_source(self):
        summary = self.workflow[self.workflow.index("## Workbench image published") :]
        for field in ("Digest", "Size", "Source revision", "Actor"):
            self.assertIn(field, summary)
        self.assertIn("$GITHUB_RUN_ID", summary)
        self.assertIn("$GITHUB_STEP_SUMMARY", summary)


if __name__ == "__main__":
    unittest.main()
