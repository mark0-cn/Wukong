import tempfile
import unittest
from pathlib import Path

from scripts.ci.check_docs import check, text_hash


class DocumentationChecks(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.files = ["README.md", "README.zh-CN.md"]
        self.write("README.md", "# Guide\n\n[简体中文](README.zh-CN.md)\n\n## Setup\n")
        self.write("README.zh-CN.md", "# 指南\n\n[English](README.md)\n\n## 安装\n")

    def write(self, path, text):
        (self.root / path).parent.mkdir(parents=True, exist_ok=True)
        (self.root / path).write_text(text, encoding="utf-8")

    def test_valid_pair_and_line_endings(self):
        errors, record = check(self.root, self.files)
        self.assertEqual([], errors)
        self.assertEqual([], check(self.root, self.files, record)[0])
        self.assertEqual(text_hash("a\r\nb"), text_hash("a\nb"))

    def test_missing_pair_and_one_sided_edits(self):
        self.assertTrue(check(self.root, ["README.md"])[0])
        _, record = check(self.root, self.files)
        with (self.root / "README.md").open("a", encoding="utf-8") as stream:
            stream.write("New behavior.\n")
        self.assertTrue(any("changed since review" in error for error in check(self.root, self.files, record)[0]))

    def test_missing_anchor_and_relative_target(self):
        with (self.root / "README.md").open("a", encoding="utf-8") as stream:
            stream.write("[missing](#absent) [file](no.md)\n")
        errors, _ = check(self.root, self.files)
        self.assertTrue(any("missing anchor" in error for error in errors))
        self.assertTrue(any("missing link target" in error for error in errors))

    def test_explicit_unicode_anchor_and_fenced_examples(self):
        with (self.root / "README.md").open("a", encoding="utf-8") as stream:
            stream.write('<a id="安装"></a>\n[setup](#%E5%AE%89%E8%A3%85)\n```md\n# example\n[not a link](missing.md)\n```\n')
        self.assertEqual([], check(self.root, self.files)[0])

    def test_new_guide_and_runtime_exclusion(self):
        self.write("docs/new.md", "# New\n")
        self.write("prompts/base.md", "# Runtime asset\n")
        errors, _ = check(self.root, self.files + ["docs/new.md", "prompts/base.md"])
        self.assertTrue(any("docs/new.md: missing counterpart" in error for error in errors))
        self.assertFalse(any("prompts/" in error for error in errors))

    def test_missing_switch_and_different_sections(self):
        self.write("README.zh-CN.md", "# 指南\n")
        errors, _ = check(self.root, self.files)
        self.assertTrue(any("reciprocal" in error for error in errors))
        self.assertTrue(any("section structure" in error for error in errors))

    def test_cross_language_navigation_and_escape(self):
        self.write("docs/other.md", "# Other\n[简体中文](other.zh-CN.md)\n")
        self.write("docs/other.zh-CN.md", "# 其他\n[English](other.md)\n")
        with (self.root / "README.md").open("a", encoding="utf-8") as stream:
            stream.write("[wrong](docs/other.zh-CN.md) [escape](../outside.md)\n")
        errors, _ = check(self.root, self.files + ["docs/other.md", "docs/other.zh-CN.md"])
        self.assertTrue(any("cross-language" in error for error in errors))
        self.assertTrue(any("leaves repository" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
