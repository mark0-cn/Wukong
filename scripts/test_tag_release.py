import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
from release import ROOT, VERSION_FILES, bump_version, version
from tag_release import git, push_tag, release_pr, validate_change, validate_ci


class ReleaseDecisionTests(unittest.TestCase):
    def setUp(self):
        self.repository = "owner/repo"
        self.sha = "a" * 40
        self.run = dict(event="push", head_branch="master", path=".github/workflows/ci.yml",
                        repository=dict(full_name=self.repository), head_repository=dict(full_name=self.repository),
                        status="completed", conclusion="success", head_sha=self.sha)
        self.jobs = [dict(name=name, conclusion="success") for name in ("changes", "windows", "linux", "summary")]
        self.pr = dict(number=12, merged=True, merge_commit_sha=self.sha,
                       base=dict(ref="master", repo=dict(full_name=self.repository)),
                       head=dict(ref="codex/release-v1.2.3", repo=dict(full_name=self.repository)),
                       user=dict(type="Bot", login="release-app[bot]"))

    def test_only_full_master_push_ci_is_accepted(self):
        validate_ci(self.run, self.jobs, self.repository)
        for field, value in [("event", "pull_request"), ("event", "workflow_dispatch"),
                             ("head_branch", "topic"), ("status", "in_progress"),
                             ("conclusion", "failure"), ("path", ".github/workflows/other.yml"),
                             ("head_sha", "bad"), ("head_repository", dict(full_name="other/repo"))]:
            changed = dict(self.run, **{field: value})
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                validate_ci(changed, self.jobs, self.repository)
        for jobs in (self.jobs[:-1], self.jobs + [self.jobs[0]],
                     [dict(j, conclusion="skipped") for j in self.jobs]):
            with self.assertRaises(ValueError):
                validate_ci(self.run, jobs, self.repository)

    def test_only_merged_app_version_pr_at_exact_ci_commit_is_selected(self):
        self.assertEqual(release_pr(self.pr, self.sha, self.repository)["tag"], "v1.2.3")
        changes = [dict(merged=False), dict(merge_commit_sha="b" * 40),
                   dict(user=dict(type="User", login="owner")),
                   dict(user=dict(type="Bot", login="github-actions[bot]")),
                   dict(head=dict(ref="codex/feature", repo=dict(full_name=self.repository))),
                   dict(head=dict(ref="codex/release-v1.2.3", repo=dict(full_name="fork/repo"))),
                   dict(base=dict(ref="develop", repo=dict(full_name=self.repository)))]
        for change in changes:
            with self.subTest(change=change):
                self.assertIsNone(release_pr(dict(self.pr, **change), self.sha, self.repository))
        bad = copy.deepcopy(self.pr)
        bad["head"]["ref"] = "codex/release-v01.2.3"
        with self.assertRaises(ValueError):
            release_pr(bad, self.sha, self.repository)


class ReleaseGitTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "repo"
        self.root.mkdir()
        git("init", "--initial-branch=master", root=self.root)
        for key, value in [("user.name", "CI fixture"), ("user.email", "ci@example.invalid"),
                           ("core.autocrlf", "false"), ("commit.gpgsign", "false"), ("tag.gpgsign", "false")]:
            git("config", key, value, root=self.root)
        for name in (*VERSION_FILES, "rust-toolchain.toml"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((ROOT / name).read_bytes().replace(b"\r\n", b"\n"))
        self.base = self.commit("base")
        self.tag = f"v{int(version(root=self.root).split('.')[0]) + 1}.0.0"
        bump_version(self.tag, self.root)

    def tearDown(self):
        self.temporary.cleanup()

    def commit(self, message):
        git("add", ".", root=self.root)
        git("commit", "-m", message, root=self.root)
        sha = git("rev-parse", "HEAD", root=self.root).decode().strip()
        git("update-ref", "refs/remotes/origin/master", sha, root=self.root)
        return sha

    def test_pure_version_bump_is_valid_even_when_master_advances(self):
        sha = self.commit("version")
        (self.root / "later.txt").write_text("later master commit")
        self.commit("later")
        validate_change(sha, self.tag, self.root)
        with self.assertRaises(ValueError):
            validate_change(sha, "v9999.0.0", self.root)

    def test_extra_file_and_hidden_manifest_change_are_rejected(self):
        (self.root / "extra.txt").write_text("not a version change")
        with self.assertRaisesRegex(ValueError, "exactly the five"):
            validate_change(self.commit("extra"), self.tag, self.root)
        git("reset", "--hard", self.base, root=self.root)
        bump_version(self.tag, self.root)
        manifest = self.root / "Cargo.toml"
        manifest.write_bytes(manifest.read_bytes() + b"\n# unrelated edit\n")
        with self.assertRaisesRegex(ValueError, "beyond the version bump"):
            validate_change(self.commit("manifest edit"), self.tag, self.root)

    def test_annotated_tag_is_idempotent_and_never_moves(self):
        sha = self.commit("version")
        remote = Path(self.temporary.name) / "remote.git"
        subprocess.check_output(["git", "init", "--bare", str(remote)], stderr=subprocess.DEVNULL)
        git("remote", "add", "origin", str(remote), root=self.root)
        git("push", "origin", "master", root=self.root)
        self.assertEqual(push_tag(self.tag, sha, self.root), "created")
        self.assertEqual(push_tag(self.tag, sha, self.root), "already-present")
        with self.assertRaisesRegex(ValueError, "Refusing to move"):
            push_tag(self.tag, self.base, self.root)
        self.assertEqual(git("rev-parse", self.tag + "^{commit}", root=self.root).decode().strip(), sha)
