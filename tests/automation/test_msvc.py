from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts._lib import msvc


class MsvcEnvironmentTests(unittest.TestCase):
    def test_diagnostics_are_limited_to_toolchain_values(self):
        environment = {
            "PATH": "C:/tools",
            "VSINSTALLDIR": "C:/VS",
            "CC": "C:/custom/cl.exe",
            "TMP": "D:/work/target/tmp",
            "OPENAI_API_KEY": "must-not-be-reported",
        }
        with patch.object(msvc.shutil, "which", return_value=None):
            result = msvc.diagnostics(environment)
        self.assertEqual(set(result), {"VSINSTALLDIR", "CC", "TMP"})
        self.assertNotIn("OPENAI_API_KEY", result)
        self.assertNotIn("PATH", result)

    def test_executable_preserves_the_rustup_cargo_proxy_name(self):
        with patch.object(msvc.shutil, "which", return_value="C:/tools/cargo.exe"):
            result = msvc.executable("cargo", {"PATH": "C:/tools"})
        self.assertEqual(result, str(Path("C:/tools/cargo.exe").absolute()))
        self.assertTrue(result.endswith("cargo.exe"))

    def test_build_environment_routes_both_temp_variables_to_the_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inherited = {"PATH": "C:/tools"}
            with patch.object(msvc, "compiler_environment", return_value=inherited):
                result = msvc.build_environment(root)
            expected = str((root / "target/tmp").resolve())
            self.assertEqual(result["TMP"], expected)
            self.assertEqual(result["TEMP"], expected)
            self.assertIs(result, inherited)

    def test_preflight_compiles_archives_and_links_with_the_same_environment(self):
        environment = {"PATH": "C:/tools", "TMP": "D:/work/target/tmp"}
        tools = {name: f"C:/tools/{name}" for name in msvc.EXECUTABLES[1:]}
        events = []

        def run(command, **kwargs):
            events.append((command, kwargs))
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(msvc, "executable", side_effect=lambda name, env: tools[name]),
            patch.object(msvc.subprocess, "run", side_effect=run),
        ):
            result = msvc.preflight(environment, Path(directory))

        self.assertEqual(result, tools)
        self.assertEqual(len(events), 3)
        self.assertTrue(all(event[1]["env"] is environment for event in events))
        self.assertIn("/c", events[0][0])
        self.assertEqual(events[1][0][0], tools["lib.exe"])
        self.assertEqual(events[2][0][0], tools["link.exe"])

    def test_preflight_reports_bounded_compiler_output(self):
        environment = {"PATH": "C:/tools"}
        failure = subprocess.CompletedProcess("cl", 2, stdout="", stderr="x" * 6000)
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(msvc, "executable", side_effect=lambda name, env: name),
            patch.object(msvc.subprocess, "run", return_value=failure),
        ):
            with self.assertRaisesRegex(ValueError, "compiler/SDK") as raised:
                msvc.preflight(environment, Path(directory))
        self.assertLess(len(str(raised.exception)), 4300)

    def test_cold_native_preflight_uses_a_fresh_target_and_cleans_success(self):
        environment = {"PATH": "C:/tools"}
        targets = []
        commands = []

        def run(command, **kwargs):
            commands.append(command)
            target = Path(command[command.index("--target-dir") + 1])
            targets.append(target)
            self.assertFalse(any(target.iterdir()))
            (target / "fresh-output").write_text("fixture", encoding="utf-8")
            self.assertEqual(kwargs["cwd"], root)
            self.assertIs(kwargs["env"], environment)
            return subprocess.CompletedProcess(command, 0)

        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(msvc, "executable", return_value="C:/tools/cargo.exe"),
            patch.object(msvc.subprocess, "run", side_effect=run),
        ):
            root = Path(directory)
            msvc.native_preflight(environment, root)

        self.assertEqual(commands[0][1:5], ["build", "--locked", "-vv", "-p"])
        for package in msvc.NATIVE_PACKAGES:
            self.assertIn(package, commands[0])
        self.assertFalse(targets[0].exists())

    def test_failed_native_preflight_retains_only_its_task_owned_target(self):
        environment = {"PATH": "C:/tools"}
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(msvc, "executable", return_value="C:/tools/cargo.exe"),
            patch.object(
                msvc.subprocess,
                "run",
                return_value=subprocess.CompletedProcess("cargo", 1),
            ),
        ):
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "retained build-script files"):
                msvc.native_preflight(environment, root)
            targets = list((root / "target/msvc-preflight").glob("native-*"))
            self.assertEqual(len(targets), 1)
            self.assertTrue(targets[0].is_dir())

    def test_explicit_native_target_must_be_empty(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "restored-cache"
            target.mkdir()
            (target / "stale").write_text("old", encoding="utf-8")
            with patch.object(msvc, "executable", return_value="cargo"):
                with self.assertRaisesRegex(ValueError, "new or empty"):
                    msvc.native_preflight({}, root, target)


if __name__ == "__main__":
    unittest.main()
