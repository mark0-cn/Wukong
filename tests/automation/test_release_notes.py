import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from scripts._lib.repository import ROOT
from scripts._lib.checksums import sha256
from scripts.release.release_notes import collect, previous_tag, render, update_summary, validate
from scripts.ci.release_preview import preview
from tests.automation.test_commit_policy import FRAGMENT


class FrozenNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.content = dict(schema=1, tag="v1.0.1", version="1.0.1", source_revision="a" * 40,
                            base_revision="b" * 40, previous_tag="v1.0.0", changelog="### Fixes\n\n- Repair progress display.",
                            entries=[dict(path=".changes/progress.toml", kind="fix", en="Show live progress.", zh_CN="展示实时进度。")])

    def accepted_assets(self):
        content_file = self.root / "release-content.json"
        content_file.write_text(json.dumps(self.content, ensure_ascii=False), encoding="utf-8")
        image = "hlmark0/wukong@sha256:" + "c" * 64
        shared = dict(source_revision="a" * 40, version="1.0.1", image=image)
        (self.root / "release.json").write_text(json.dumps(shared))
        (self.root / "validation-summary.json").write_text(json.dumps(dict(passed=True, **shared)))
        updater = dict(notes=update_summary(self.content), release_content_sha256=sha256(content_file), platforms={}, **shared)
        for platform, name in [("windows-x86_64", "Wukong_1.0.1_x64-setup.exe"), ("linux-x86_64", "Wukong_1.0.1_amd64.AppImage")]:
            path = self.root / name
            path.write_bytes(b"accepted installer bytes")
            updater["platforms"][platform] = dict(url="https://github.com/mark0-cn/Wukong/releases/download/v1.0.1/" + name,
                                                size=path.stat().st_size, sha256=sha256(path))
        (self.root / "latest.json").write_text(json.dumps(updater), encoding="utf-8")

    def test_preview_never_claims_acceptance_or_published_downloads(self):
        body = render(self.content)
        self.assertIn("Show live progress.", body)
        self.assertIn("展示实时进度。", body)
        self.assertIn("Preview only", body)
        self.assertNotIn("acceptance passed", body)
        self.assertNotIn("/releases/download/", body)

    def test_accepted_page_uses_actual_assets_and_is_independent_of_later_git_edits(self):
        self.accepted_assets()
        with patch("scripts.release.release_notes.git", side_effect=AssertionError("Do not reread mutable history")):
            body = render(self.content, directory=self.root)
        self.assertIn("Wukong_1.0.1_x64-setup.exe", body)
        self.assertIn("Wukong_1.0.1_amd64.AppImage", body)
        self.assertIn("acceptance passed", body)
        self.assertIn("Authenticode", body)
        self.assertIn("简体中文", body)
        self.assertNotIn("Preview only", body)

    def test_mismatched_identity_signed_notes_or_asset_bytes_are_rejected(self):
        self.accepted_assets()
        with self.assertRaises(ValueError):
            validate(self.content, "d" * 40, "1.0.1")
        updater = json.loads((self.root / "latest.json").read_text(encoding="utf-8"))
        updater["notes"] = "Changed after signing"
        (self.root / "latest.json").write_text(json.dumps(updater))
        with self.assertRaisesRegex(ValueError, "signed updater"):
            render(self.content, directory=self.root)
        self.accepted_assets()
        (self.root / "release-content.json").write_text("changed")
        with self.assertRaisesRegex(ValueError, "signed updater"):
            render(self.content, directory=self.root)
        self.accepted_assets()
        next(self.root.glob("*.exe")).write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "Installer"):
            render(self.content, directory=self.root)

    def test_failed_acceptance_cannot_generate_a_success_page(self):
        self.accepted_assets()
        report = self.root / "validation-summary.json"
        data = json.loads(report.read_text())
        data["passed"] = False
        report.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "acceptance"):
            render(self.content, directory=self.root)

    def test_updater_summary_is_bounded_without_truncating_full_release_notes(self):
        self.content["entries"] = [dict(path=f".changes/change-{index}.toml", kind="feature",
                                        en="Describe concrete behavior. " * 60, zh_CN="完整的中文变化说明。" * 100) for index in range(50)]
        summary = update_summary(self.content)
        self.assertLessEqual(len(summary.encode("utf-8")), 32 * 1024)
        self.assertIn("Full release notes", summary)
        self.assertIn("完整发布说明", summary)
        self.assertEqual(len(self.content["entries"]), 50)
        self.assertNotIn("more changes", render(self.content))


@unittest.skipUnless((ROOT / "node_modules/git-cliff/lib/cli/cli.js").is_file(), "npm ci required; preview CI exercises real git-cliff")
class RealGitCliffTests(unittest.TestCase):
    def test_tag_range_bilingual_fragments_and_breaking_changes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            def run(*args):
                return subprocess.check_output(["git", *args], cwd=root).decode().strip()
            def commit(message):
                run("add", ".")
                run("-c", "core.hooksPath=", "commit", "--allow-empty", "-qm", message)
                return run("rev-parse", "HEAD")
            run("init", "-q")
            run("config", "user.name", "Fixture")
            run("config", "user.email", "fixture@example.invalid")
            (root / "Cargo.toml").write_text('[package]\nversion = "1.0.0"\n')
            old = commit("feat(ui): historical entry outside release range")
            run("tag", "v1.0.0")
            (root / ".changes").mkdir()
            (root / ".changes/progress.toml").write_text(FRAGMENT, encoding="utf-8")
            commit("feat(ui): display live tool progress")
            commit("chore(config)!: replace storage keys\n\nBREAKING CHANGE: migrate storage keys in settings")
            (root / "Cargo.toml").write_text('[package]\nversion = "1.1.0"\n')
            head = commit("release: prepare v1.1.0")
            run("tag", "v1.1.0")
            self.assertEqual(previous_tag("v1.1.0", head, root), "v1.0.0")
            content = collect("v1.1.0", head, root=root)
            self.assertEqual(content["base_revision"], old)
            self.assertEqual(content["source_revision"], head)
            self.assertEqual(content["entries"][0]["zh_CN"], "展示工具实时进度。")
            self.assertIn("display live tool progress", content["changelog"])
            self.assertIn("BREAKING", content["changelog"])
            self.assertIn("migrate storage keys", content["changelog"])
            self.assertNotIn("historical entry", content["changelog"])
            self.assertNotIn("prepare v1.1.0", content["changelog"])
            pr = dict(base=dict(sha=old), head=dict(sha=head, ref="codex/release-v1.1.0"))
            output = root / "preview"
            summary = preview(pr, output, root=root)
            self.assertIn("Cumulative version changes", summary)
            self.assertIn(head, summary)
            self.assertEqual(json.loads((output / "next-release.json").read_text(encoding="utf-8"))["source_revision"], head)
            self.assertTrue((output / "pr-changes.md").is_file())
            pr["head"]["ref"] = "codex/ui-progress"
            self.assertIn("Changes in this PR", preview(pr, output, root=root))
            with self.assertRaises(ValueError):
                collect("v01.1.0", head, root=root)
