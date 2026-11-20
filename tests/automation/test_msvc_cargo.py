import contextlib
import io
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts.dev import msvc_cargo


class MsvcCargoTests(unittest.TestCase):
    def test_diagnostics_use_stderr_and_preserve_command_stdout(self):
        environment = {"PATH": "C:/tools"}
        argv = ["msvc_cargo", "--", "run", "--example", "export_catalogs"]
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            patch.object(msvc_cargo.os, "name", "nt"),
            patch.object(sys, "argv", argv),
            patch.object(msvc_cargo.msvc, "build_environment", return_value=environment),
            patch.object(msvc_cargo.msvc, "diagnostics", return_value={"cargo": "cargo.exe"}),
            patch.object(msvc_cargo.msvc, "preflight"),
            patch.object(msvc_cargo.msvc, "executable", return_value="cargo.exe"),
            patch.object(
                msvc_cargo.subprocess,
                "run",
                return_value=subprocess.CompletedProcess("cargo", 0),
            ) as run,
            contextlib.redirect_stdout(stdout),
            contextlib.redirect_stderr(stderr),
        ):
            self.assertEqual(msvc_cargo.main(), 0)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("MSVC environment:", stderr.getvalue())
        self.assertIn("Running:", stderr.getvalue())
        run.assert_called_once_with(
            ["cargo.exe", "run", "--example", "export_catalogs"], env=environment
        )


if __name__ == "__main__":
    unittest.main()
