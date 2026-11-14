"""Protocol definitions participate in the signed native build identity."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker import source_identity


class WorkerSourceIdentityTests(unittest.TestCase):
    def test_analysis_changes_identity_but_linux_deployment_and_build_outputs_do_not(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            analysis = root / "workers/analysis/wukong_analysis/seccomp.py"
            linux = root / "workers/runtime/worker_jobs.py"
            generated = root / "workers/analysis/build/lib/wukong_analysis/seccomp.py"
            for path in (analysis, linux, generated):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("original", encoding="utf-8")
            with patch.object(source_identity, "ROOT", root), patch.object(
                source_identity.subprocess, "check_output", return_value="a" * 40
            ):
                before = source_identity.identity()["source_tree_sha256"]
                linux.write_text("Linux deployment change", encoding="utf-8")
                generated.write_text("build output", encoding="utf-8")
                self.assertEqual(before, source_identity.identity()["source_tree_sha256"])
                analysis.write_text("shared analysis change", encoding="utf-8")
                self.assertNotEqual(before, source_identity.identity()["source_tree_sha256"])

    def test_public_contract_edit_changes_native_source_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            contract = root / "workers/protocol/src/lib.rs"
            contract.parent.mkdir(parents=True)
            contract.write_text("pub const PROTOCOL: u32 = 2;", encoding="utf-8")
            with patch.object(source_identity, "ROOT", root), patch.object(
                source_identity.subprocess, "check_output", return_value="a" * 40
            ):
                before = source_identity.identity()["source_tree_sha256"]
                contract.write_text("pub const PROTOCOL: u32 = 3;", encoding="utf-8")
                self.assertNotEqual(before, source_identity.identity()["source_tree_sha256"])

    def test_vendor_patch_edit_changes_source_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vendor_patch = root / "workers/windows/patches/debugger.patch"
            vendor_patch.parent.mkdir(parents=True)
            vendor_patch.write_text("original patch", encoding="utf-8")
            with patch.object(source_identity, "ROOT", root), patch.object(
                source_identity.subprocess, "check_output", return_value="a" * 40
            ):
                before = source_identity.identity()["source_tree_sha256"]
                vendor_patch.write_text("modified patch", encoding="utf-8")
                after = source_identity.identity()["source_tree_sha256"]
            self.assertNotEqual(before, after)

    def test_protocol_edit_changes_source_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            schema = root / "workers/windows/proto/session.proto"
            schema.parent.mkdir(parents=True)
            schema.write_text('syntax = "proto3"; message Request {}', encoding="utf-8")
            with patch.object(source_identity, "ROOT", root), patch.object(
                source_identity.subprocess, "check_output", return_value="a" * 40
            ):
                before = source_identity.identity()["source_tree_sha256"]
                schema.write_text('syntax = "proto3"; message Request { uint32 schema = 1; }', encoding="utf-8")
                after = source_identity.identity()["source_tree_sha256"]
            self.assertNotEqual(before, after)
