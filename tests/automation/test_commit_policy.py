import subprocess
import tempfile
from pathlib import Path
import unittest
from unittest.mock import patch
from scripts.ci import commit_policy
from scripts._lib.repository import ROOT
from scripts.release.note_entries import fragments, parse_fragment, pr_note


FRAGMENT = 'kind = "feature"\nen = "Show live tool progress."\nzh_CN = "展示工具实时进度。"\n'


class SummaryTests(unittest.TestCase):
    def test_requires_complete_language_pair_and_known_kind(self):
        self.assertEqual(parse_fragment(FRAGMENT, ".changes/live-progress.toml")["en"], "Show live tool progress.")
        for text, path in [(FRAGMENT.replace('kind = "feature"', 'kind = "other"'), ".changes/note.toml"),
                           (FRAGMENT.replace("Show live tool progress.", "TODO"), ".changes/note.toml"),
                           (FRAGMENT.replace("展示工具实时进度。", "English only"), ".changes/note.toml"),
                           (FRAGMENT, ".changes/../outside.toml")]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_fragment(text, path)

    def test_pr_body_needs_no_chinese_and_internal_choice_is_explicit(self):
        self.assertFalse(pr_note(dict(body="English summary only"), [{"path": ".changes/live-progress.toml"}])["internal"])
        self.assertTrue(pr_note(dict(body="<!-- release-note:none: Internal naming cleanup without user impact. -->"), [])["internal"])
        for body in ("", "<!-- release-note:none: no -->"):
            with self.assertRaises(ValueError):
                pr_note(dict(body=body), [])
        with self.assertRaises(ValueError):
            pr_note(dict(body="<!-- release-note:none: Internal change without visible behavior. -->"), [{"path": "a"}])

    def test_known_automation_is_narrowly_recognized(self):
        pr = dict(title="release: prepare v1.2.3", head=dict(ref="codex/release-v1.2.3"))
        self.assertTrue(pr_note(pr, [])["internal"])
        with self.assertRaises(ValueError):
            pr_note(dict(title=pr["title"], head=dict(ref="other")), [])
        bot = dict(title="build(deps): bump filelock from 3 to 4", user=dict(login="dependabot[bot]"))
        self.assertTrue(pr_note(bot, [])["internal"])
        with self.assertRaises(ValueError):
            pr_note(dict(title="fix(ui): repair progress rendering", user=bot["user"]), [])

    def test_commit_range_skips_real_merges_and_lints_title(self):
        pr = dict(title="fix(ui): show tool progress", body="English summary only",
                  base=dict(sha="a" * 40), head=dict(sha="b" * 40))
        with patch.object(commit_policy, "git", side_effect=["c" * 40, "fix(ui): show tool progress", "a" * 40]), \
                patch.object(commit_policy, "lint") as lint, patch.object(commit_policy, "fragments", return_value=[{"path": "a"}]):
            commit_policy.validate(pr)
            self.assertEqual(lint.call_count, 2)
        pr["head"]["sha"] = "HEAD;bad"
        with self.assertRaises(ValueError):
            commit_policy.validate(pr)

    def test_git_tree_selects_only_new_fragments_and_rejects_history_edits(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            def run(*args):
                return subprocess.check_output(["git", *args], cwd=root).decode().strip()
            run("init", "-q")
            run("config", "user.name", "Fixture")
            run("config", "user.email", "fixture@example.invalid")
            run("-c", "core.hooksPath=", "commit", "--allow-empty", "-qm", "test: initialize release fixture")
            base = run("rev-parse", "HEAD")
            (root / ".changes").mkdir()
            file = root / ".changes/note.toml"
            file.write_text(FRAGMENT, encoding="utf-8")
            run("add", ".")
            run("-c", "core.hooksPath=", "commit", "-qm", "feat(ui): show live progress")
            head = run("rev-parse", "HEAD")
            self.assertEqual(len(fragments(base, head, root=root)), 1)
            self.assertEqual(fragments(head, head, root=root), [])
            file.write_text(FRAGMENT.replace("Show", "Display"), encoding="utf-8")
            run("add", ".")
            run("-c", "core.hooksPath=", "commit", "-qm", "docs: alter old release text")
            with self.assertRaisesRegex(ValueError, "append-only"):
                fragments(head, run("rev-parse", "HEAD"), root=root)


@unittest.skipUnless((ROOT / "node_modules/@commitlint/cli/lib/cli.js").is_file(), "npm ci required; commit-policy CI installs dependencies")
class RealCommitlintTests(unittest.TestCase):
    def test_conventions_and_automation(self):
        for message in ("fix(updater): locate Docker credential helpers on Windows", "release: prepare v1.2.3",
                        "build(deps): bump filelock from 3.20.0 to 4.2.9", "feat(config)!: replace legacy storage keys\n\nBREAKING CHANGE: migrate storage keys in settings"):
            with self.subTest(message=message):
                commit_policy.lint(message)
        for message in ("fix bugs", "fix: update", "feat: improvements", "Merge pull request #1 from forged", "unknown(ui): show progress",
                        "feat(config)!: replace storage keys without migration details"):
            with self.subTest(message=message), self.assertRaises(subprocess.CalledProcessError):
                commit_policy.lint(message)
