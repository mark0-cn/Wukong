import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts._lib.checksums import sha256
from scripts.release import worker_setup


class WorkerSetupTests(unittest.TestCase):
    def setUp(self):
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(
            self.stack.enter_context(tempfile.TemporaryDirectory(prefix="worker setup "))
        )
        self.stack.enter_context(patch.object(worker_setup, "ROOT", self.root))
        programs = self.root / "Program Files (x86)"
        self.vswhere = programs / "Microsoft Visual Studio/Installer/vswhere.exe"
        self.vswhere.parent.mkdir(parents=True)
        self.vswhere.write_text("fixture vswhere")
        self.installation = programs / "Visual Studio/2022/Professional"
        self.setup = self.installation / "Common7/Tools/VsDevCmd.bat"
        self.setup.parent.mkdir(parents=True)
        self.setup.write_text("fixture dev command prompt")
        self.stack.enter_context(
            patch.dict(
                os.environ,
                {"ProgramFiles(x86)": str(programs), "VSINSTALLDIR": ""},
                clear=False,
            )
        )
        self.output = self.root / "target/worker-setup"
        self.cache = self.root / "target/windows-offline-cache"
        self.native = self.root / (
            "target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-worker.exe"
        )

    def staging_subprocess(self, events):
        def check_output(command, **kwargs):
            events.append(("vswhere", command))
            self.assertEqual(kwargs, {"text": True})
            return str(self.installation)

        def run(command, **kwargs):
            events.append(("run", command, kwargs))
            if isinstance(command, str):
                environment = (
                    "Path=" + str(self.root / "msvc/bin") + "\r\n"
                    "VSINSTALLDIR=" + str(self.installation) + "\r\n"
                )
                return subprocess.CompletedProcess(
                    command, 0, stdout=environment.encode("utf-16-le"), stderr=b""
                )
            self.assertEqual(
                command,
                [
                    sys.executable,
                    "-m",
                    "scripts.worker.build_native",
                    "--cache",
                    str(self.cache.resolve()),
                ],
            )
            self.native.parent.mkdir(parents=True, exist_ok=True)
            self.native.write_bytes(b"native worker bytes")
            return subprocess.CompletedProcess(command, 0)

        return check_output, run

    def test_build_and_stage_initializes_msvc_builds_then_copies(self):
        events = []
        check_output, run = self.staging_subprocess(events)

        def toolchain(cache):
            events.append(("toolchain", cache, {}))
            return {"assets": 1, "bytes": 2}

        with (
            patch.object(worker_setup.subprocess, "check_output", side_effect=check_output),
            patch.object(worker_setup.subprocess, "run", side_effect=run),
            patch.object(worker_setup, "acquire_toolchain", side_effect=toolchain),
        ):
            result = worker_setup.build_and_stage(self.output, self.cache)

        self.assertEqual([event[0] for event in events], ["vswhere", "run", "toolchain", "run"])
        vswhere = events[0][1]
        self.assertEqual(vswhere[0], str(self.vswhere))
        self.assertIn("Microsoft.VisualStudio.Component.VC.Tools.x86.x64", vswhere)
        self.assertIn(str(self.setup), events[1][1])
        self.assertEqual(events[1][2], {"stdout": subprocess.PIPE, "stderr": subprocess.PIPE})
        self.assertEqual(events[2][1], self.cache.resolve())
        build_environment = events[3][2]
        self.assertEqual(build_environment["cwd"], self.root)
        self.assertTrue(build_environment["check"])
        self.assertEqual(build_environment["env"]["PATH"], str(self.root / "msvc/bin"))
        self.assertEqual(build_environment["env"]["VSINSTALLDIR"], str(self.installation))

        staged = self.output / worker_setup.SETUP_NAME
        self.assertEqual(staged.read_bytes(), b"native worker bytes")
        self.assertEqual(result["executable"], str(staged.resolve()))
        self.assertEqual(result["sha256"], sha256(staged))
        self.assertEqual(result["size"], len(b"native worker bytes"))
        self.assertEqual(result["toolchain"], {"assets": 1, "bytes": 2})

    def test_missing_vswhere_reports_msvc_unavailable(self):
        self.vswhere.unlink()
        with patch.object(worker_setup.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "MSVC x64 build tools are unavailable"):
                worker_setup.build_and_stage(self.output, self.cache)
        run.assert_not_called()

    def test_missing_dev_command_prompt_reports_msvc_unavailable(self):
        self.setup.unlink()
        with (
            patch.object(
                worker_setup.subprocess, "check_output", return_value=str(self.installation)
            ),
            patch.object(worker_setup.subprocess, "run") as run,
        ):
            with self.assertRaisesRegex(ValueError, "MSVC x64 build tools are unavailable"):
                worker_setup.build_and_stage(self.output, self.cache)
        run.assert_not_called()

    def test_toolchain_failure_stops_before_building(self):
        events = []
        check_output, run = self.staging_subprocess(events)
        with (
            patch.object(worker_setup.subprocess, "check_output", side_effect=check_output),
            patch.object(worker_setup.subprocess, "run", side_effect=run),
            patch.object(
                worker_setup,
                "acquire_toolchain",
                side_effect=ValueError("Missing offline payload (controller download required)"),
            ),
        ):
            with self.assertRaisesRegex(ValueError, "Missing offline payload"):
                worker_setup.build_and_stage(self.output, self.cache)
        self.assertEqual([event[0] for event in events], ["vswhere", "run"])
        self.assertFalse((self.output / worker_setup.SETUP_NAME).exists())

    def test_acquire_toolchain_limits_assets_to_the_native_build(self):
        lock = {
            "assets": [
                {"component": "windbg_mcp", "path": "windbg-mcp-source.zip"},
                {"component": "python", "path": "python.zip"},
            ]
        }
        with (
            patch.object(worker_setup.toolchain, "load_lock", return_value=lock),
            patch.object(worker_setup.toolchain, "acquire", return_value={"assets": 1}) as acquire,
        ):
            result = worker_setup.acquire_toolchain(self.cache)
        self.assertEqual(result, {"assets": 1})
        cache, prepared = acquire.call_args.args
        self.assertEqual(cache, self.cache)
        self.assertEqual([item["component"] for item in prepared["assets"]], ["windbg_mcp"])
        self.assertEqual(acquire.call_args.kwargs, {"download": True})

    def test_toolchain_status_distinguishes_missing_assets(self):
        lock = {"assets": [{"component": "windbg_mcp", "path": "windbg-mcp-source.zip"}]}
        with (
            patch.object(worker_setup.toolchain, "load_lock", return_value=lock),
            patch.object(worker_setup.toolchain, "acquire", return_value={}) as acquire,
        ):
            self.assertEqual(
                worker_setup.toolchain_status(self.cache),
                {
                    "available": False,
                    "download_required": True,
                    "missing": ["windbg-mcp-source.zip"],
                },
            )
            acquire.assert_not_called()
            (self.cache / "windbg-mcp-source.zip").parent.mkdir(parents=True)
            (self.cache / "windbg-mcp-source.zip").write_bytes(b"fixture")
            self.assertEqual(
                worker_setup.toolchain_status(self.cache),
                {"available": True, "download_required": False, "missing": []},
            )
        acquire.assert_called_once()
        cache, prepared = acquire.call_args.args
        self.assertEqual(cache, self.cache)
        self.assertEqual(prepared, {"assets": lock["assets"]})

    def test_check_validates_entry_points_without_building(self):
        lock = {"assets": [{"component": "windbg_mcp", "path": "windbg-mcp-source.zip"}]}
        environment = {
            "PATH": str(self.root / "msvc/bin"),
            "VSINSTALLDIR": str(self.installation),
        }
        source = self.root / "workers/windows"
        source.mkdir(parents=True)
        (source / "Cargo.lock").write_text("fixture")
        with (
            patch.object(worker_setup, "build_environment", return_value=environment),
            patch.object(
                worker_setup.shutil, "which", side_effect=lambda name, path=None: name
            ) as which,
            patch.object(worker_setup.toolchain, "load_lock", return_value=lock),
            patch.object(worker_setup.subprocess, "run") as run,
        ):
            details = worker_setup.check(self.cache)
        self.assertEqual(which.call_count, len(worker_setup.EXECUTABLES))
        run.assert_not_called()
        self.assertEqual(details["msvc_installation"], str(self.installation))
        self.assertEqual(details["native_source"], str(source))
        self.assertTrue(details["toolchain"]["download_required"])

    def test_cli_stage_publishes_github_identity(self):
        summary = self.root / "summary.md"
        output = self.root / "github-output"
        executable = self.output / worker_setup.SETUP_NAME
        result = {"executable": str(executable), "sha256": "a" * 64, "size": 7, "toolchain": {}}
        argv = [
            "worker_setup",
            "--output",
            str(self.output),
            "--cache",
            str(self.cache),
        ]
        stdout = io.StringIO()
        with (
            patch.object(sys, "argv", argv),
            patch.object(worker_setup, "build_and_stage", return_value=result) as stage,
            patch.dict(
                os.environ,
                {"GITHUB_STEP_SUMMARY": str(summary), "GITHUB_OUTPUT": str(output)},
                clear=False,
            ),
            contextlib.redirect_stdout(stdout),
        ):
            worker_setup.main()
        stage.assert_called_once_with(self.output, self.cache)
        self.assertEqual(
            summary.read_text(encoding="utf-8"),
            "WukongWorkerSetup.exe staged in target/worker-setup: "
            "7 bytes, SHA256 " + "a" * 64 + ".\n",
        )
        self.assertEqual(output.read_text(encoding="utf-8"), "sha256=" + "a" * 64 + "\nsize=7\n")
        self.assertEqual(json.loads(stdout.getvalue()), result)


if __name__ == "__main__":
    unittest.main()
