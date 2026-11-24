import subprocess
from pathlib import Path
import unittest
from unittest.mock import patch

from scripts.ci import playwright_deps


class PlaywrightDependencyTests(unittest.TestCase):
    def test_missing_packages_use_the_bounded_apt_installer(self):
        result = subprocess.CompletedProcess(
            [],
            1,
            stdout=(
                "Playwright host validation warning\n"
                "Missing system dependencies (2):\n"
                "  libasound2t64\n"
                "  libatk1.0-0t64\n"
            ),
            stderr="",
        )
        with (
            patch.object(playwright_deps.subprocess, "run", return_value=result) as run,
            patch.object(playwright_deps.install_apt, "install") as install,
        ):
            playwright_deps.install("chromium")
        self.assertEqual(
            run.call_args.args[0],
            [
                "npx",
                "--no-install",
                "playwright",
                "install-deps",
                "--dry-run",
                "chromium",
            ],
        )
        self.assertEqual(run.call_args.kwargs["timeout"], 60)
        install.assert_called_once_with(["libasound2t64", "libatk1.0-0t64"])

    def test_preinstalled_dependencies_skip_apt(self):
        result = subprocess.CompletedProcess(
            [], 0, stdout="All system dependencies are installed.\n", stderr=""
        )
        with (
            patch.object(playwright_deps.subprocess, "run", return_value=result),
            patch.object(playwright_deps.install_apt, "install") as install,
        ):
            playwright_deps.install("chromium")
        install.assert_not_called()

    def test_probe_errors_and_malformed_results_fail_closed(self):
        cases = [
            (2, "", "apt-get could not simulate installation"),
            (1, "apt-get could not simulate installation\n", ""),
            (1, "Missing system dependencies (1):\nnot a package line\n", ""),
            (0, "unrecognized success output\n", ""),
        ]
        for returncode, stdout, stderr in cases:
            with self.subTest(returncode=returncode, stdout=stdout):
                result = subprocess.CompletedProcess([], returncode, stdout=stdout, stderr=stderr)
                with patch.object(playwright_deps.subprocess, "run", return_value=result):
                    with self.assertRaises(RuntimeError):
                        playwright_deps.missing_packages("chromium")

    def test_ci_uses_failover_for_packages_but_not_browser_download(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("python -m scripts.ci.playwright_deps chromium", workflow)
        self.assertIn("npx --no-install playwright install chromium", workflow)
        self.assertNotIn("playwright install --with-deps", workflow)


if __name__ == "__main__":
    unittest.main()
