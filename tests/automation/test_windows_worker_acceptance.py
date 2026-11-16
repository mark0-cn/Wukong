"""The global installation fixture must reject a developer host before effects."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "hosted_worker_fixture", Path(__file__).parents[1] / "acceptance/check_windows_worker.py"
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class HostedWorkerGuardTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("pwsh"), "PowerShell diagnostic parser requires pwsh")
    def test_restart_accepts_current_ready_summary_and_rejects_failures_and_stale_identity(self):
        import copy
        import json

        helper = Path(__file__).parents[1] / "acceptance/windows-worker-startup.ps1"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            metadata = root / "metadata"
            metadata.mkdir()
            stages = [
                "deployment_identity",
                "resource_recovery",
                "dependency_selftest",
                "guest_controls_selftest",
                "communication_ready",
            ]
            value = dict(
                schema=2,
                installation_id="installation",
                package_sha256="a" * 64,
                attempt_id="b" * 32,
                ready=True,
                execution_boundary="disposable_vm",
                external_isolation_verified=False,
                events=[
                    dict(stage=stage, status=status)
                    for stage in stages
                    for status in ["started", "succeeded"]
                ],
            )
            runner = root / "read-startup.ps1"
            runner.write_text(
                "param($Helper,$Root)\n$ErrorActionPreference='Stop'\n. $Helper\n"
                "Read-ReadyWorkerStartup $Root 'installation' ('a'*64) | ConvertTo-Json -Compress\n",
                encoding="utf-8",
            )
            summary = (
                f"attempt={'b' * 32} installation=installation code=startup_ready elapsed_ms=1\n"
            )

            def inspect(report, text):
                (metadata / "startup-diagnostics.json").write_text(
                    json.dumps(report), encoding="utf-8"
                )
                (metadata / "last-service-error.txt").write_text(text, encoding="utf-8")
                return subprocess.run(
                    [
                        shutil.which("pwsh"),
                        "-NoProfile",
                        "-NonInteractive",
                        "-File",
                        str(runner),
                        str(helper.resolve()),
                        str(root),
                    ],
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    timeout=20,
                )

            result = inspect(value, summary)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["attempt_id"], value["attempt_id"])
            for text in [
                summary.replace("b" * 32, "c" * 32),
                summary.replace("installation=installation", "installation=other"),
                summary.replace("startup_ready", "service_runtime_failed"),
                summary.replace("startup_ready", "startup_pending"),
                summary + "x" * 1025,
            ]:
                with self.subTest(summary=text[:120]):
                    self.assertNotEqual(inspect(value, text).returncode, 0)
            for change in [
                {"package_sha256": "c" * 64},
                {"ready": False},
                {"execution_boundary": "other"},
                {"external_isolation_verified": True},
                {"events": value["events"][:-1]},
                {"oversized": "x" * 16385},
            ]:
                with self.subTest(fields=list(change)):
                    self.assertNotEqual(inspect({**value, **change}, summary).returncode, 0)
            failed = copy.deepcopy(value)
            failed["events"][-1]["status"] = "failed"
            self.assertNotEqual(inspect(failed, summary).returncode, 0)

    def test_startup_report_identity_bounds_stages_and_summary_are_validated(self):
        import json

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = root / "metadata"
            metadata.mkdir()
            value = dict(
                schema=2,
                installation_id="installation",
                package_sha256="a" * 64,
                boot_id="boot",
                attempt_id="b" * 32,
                started_at_ms=1,
                ready=False,
                events=[
                    dict(
                        stage="deployment_identity",
                        status="started",
                        elapsed_ms=0,
                        code="stage_started",
                        summary="Startup stage started.",
                    ),
                    dict(
                        stage="deployment_identity",
                        status="failed",
                        elapsed_ms=1,
                        code="deployment_identity_failed",
                        summary="Identity failed.",
                    ),
                ],
                execution_boundary="disposable_vm",
                external_isolation_verified=False,
            )
            report = metadata / "startup-diagnostics.json"
            report.write_text(json.dumps(value), encoding="utf-8")
            summary = metadata / "last-service-error.txt"
            summary.write_text(
                "attempt=" + "b" * 32 + " code=deployment_identity_failed", encoding="utf-8"
            )
            self.assertEqual(fixture.startup_diagnostic(root, "installation", "a" * 64), value)
            with self.assertRaisesRegex(ValueError, "installation mismatch"):
                fixture.startup_diagnostic(root, "other", "a" * 64)
            with self.assertRaisesRegex(ValueError, "package mismatch"):
                fixture.startup_diagnostic(root, "installation", "c" * 64)
            summary.write_text("old attempt", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "another attempt"):
                fixture.startup_diagnostic(root)
            report.write_text("x" * 16385, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exceeds bound"):
                fixture.startup_diagnostic(root)
            value["external_isolation_verified"] = True
            report.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "boundary mismatch"):
                fixture.startup_diagnostic(root)

    def test_only_explicit_bounded_fault_sections_enter_the_report(self):
        private = b"UNREQUESTED-REGISTER-CANARY"
        sections = fixture.debugger_fault_sections(
            private
            + b"\r\nWUKONG_FAULT\r\n"
            + b"f" * 5000
            + b"\r\nWUKONG_MODULES\r\n"
            + b"m" * 9000
        )
        self.assertEqual(sections, dict(fault="f" * 4096, modules="m" * 8192))
        for output in [private, private + b"\nWUKONG_FAULT\nno closing marker"]:
            self.assertEqual(fixture.debugger_fault_sections(output), dict(fault="", modules=""))

    def test_crash_diagnostics_cannot_spawn_on_a_developer_host(self):
        with (
            patch.dict(os.environ, {}, clear=True),
            patch.object(fixture.platform, "system", return_value="Windows"),
            patch.object(fixture.subprocess, "run", side_effect=AssertionError("host execution")),
        ):
            with self.assertRaisesRegex(RuntimeError, "dedicated GitHub-hosted"):
                fixture.diagnose_sshd_crash(None, None, None, None, True)

    def test_network_denial_exempts_only_the_enrolled_local_address_and_loopback(self):
        import ipaddress

        for management in ["10.0.0.7", "192.0.2.8", "172.16.0.2"]:
            ranges = [
                tuple(map(ipaddress.ip_address, item.split("-")))
                for item in fixture.external_addresses(management)
            ]

            def blocked(text):
                address = ipaddress.ip_address(text)
                return any(
                    low.version == address.version and low <= address <= high
                    for low, high in ranges
                )

            self.assertFalse(blocked(management))
            for address in ["127.0.0.1", "127.255.255.255", "::1"]:
                self.assertFalse(blocked(address))
            for address in [
                "10.0.0.8",
                "192.0.2.9",
                "1.1.1.1",
                "126.255.255.255",
                "128.0.0.0",
                "255.255.255.255",
                "::2",
                "2001:db8::1",
            ]:
                self.assertTrue(blocked(address), address)
        for invalid in ["127.0.0.1", "0.0.0.0", "224.0.0.1", "::1"]:
            with self.assertRaises(ValueError):
                fixture.external_addresses(invalid)

    def test_public_diagnostics_exclude_private_records_and_bound_trusted_logs(self):
        import json

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = root / "metadata"
            metadata.mkdir()
            for name in [
                "endpoint.json",
                "exclusive-lease.json",
                "authority.json",
                "ssh_host_ed25519_key",
                "install-tool-unreviewed.log",
            ]:
                (metadata / name).write_text(
                    "CONTROLLED-PRIVATE-CANARY-NOT-A-CREDENTIAL", encoding="utf-8"
                )
            (metadata / "last-service-error.txt").write_text(
                "known fixture failure", encoding="utf-8"
            )
            (metadata / "selftest-fixture.log").write_text("x" * 9000, encoding="utf-8")
            (metadata / "install-tool-sshd.log").write_text("y" * 9000, encoding="utf-8")
            (metadata / "install-tool-ssh-keygen.log").write_text(
                "trusted keygen status", encoding="utf-8"
            )
            report = fixture.diagnostics(root)
            self.assertEqual(report["last-service-error.txt"], "known fixture failure")
            self.assertEqual(len(report["selftest-fixture.log"]), 4096)
            self.assertEqual(len(report["install-tool-sshd.log"]), 4096)
            self.assertEqual(report["install-tool-ssh-keygen.log"], "trusted keygen status")
            self.assertNotIn("CONTROLLED-PRIVATE-CANARY", json.dumps(report))

    def test_default_and_self_hosted_environments_cannot_reach_installation_effects(self):
        for environment in [{}, {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "self-hosted"}]:
            with (
                self.subTest(environment=environment),
                patch.dict(os.environ, environment, clear=True),
                patch.object(fixture.platform, "system", return_value="Windows"),
                patch.object(
                    fixture.tempfile, "mkdtemp", side_effect=AssertionError("host mutation")
                ),
                patch.object(
                    sys,
                    "argv",
                    ["fixture", "--confirm-hosted-runner", "--report", "never-created.json"],
                ),
            ):
                with self.assertRaisesRegex(RuntimeError, "dedicated GitHub-hosted"):
                    fixture.main()

    def test_explicit_confirmation_and_windows_are_required_even_with_hosted_environment(self):
        with tempfile.TemporaryDirectory() as temporary:
            environment = {
                "GITHUB_ACTIONS": "true",
                "RUNNER_ENVIRONMENT": "github-hosted",
                "RUNNER_TEMP": temporary,
            }
            with (
                patch.dict(os.environ, environment, clear=True),
                patch.object(fixture.platform, "system", return_value="Windows"),
            ):
                with self.assertRaises(RuntimeError):
                    fixture.require_hosted(False)
                self.assertEqual(fixture.require_hosted(True), Path(temporary).resolve())
            with (
                patch.dict(os.environ, environment, clear=True),
                patch.object(fixture.platform, "system", return_value="Linux"),
            ):
                with self.assertRaises(RuntimeError):
                    fixture.require_hosted(True)
