"""Bootstrap dependency validation rejects dynamic CRTs and malformed import tables."""
from pathlib import Path
import struct
import json
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker.pe import imported_libraries, require_static_crt


def fixture(library, delay=False, architecture="x64"):
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 128)
    data[128:132] = b"PE\0\0"
    struct.pack_into("<HH", data, 132, 0x8664 if architecture == "x64" else 0x14c, 1)
    struct.pack_into("<H", data, 148, 240)
    optional = 152
    struct.pack_into("<H", data, optional, 0x20b if architecture == "x64" else 0x10b)
    struct.pack_into("<I", data, optional + 60, 512)
    directory_base = 112 if architecture == "x64" else 96
    struct.pack_into("<I", data, optional + directory_base - 4, 16)
    struct.pack_into("<II", data, optional + directory_base + (13 if delay else 1) * 8, 4096, 64 if delay else 40)
    struct.pack_into("<IIII", data, optional + 240 + 8, 512, 4096, 512, 512)
    struct.pack_into("<I", data, 512 + (4 if delay else 12), 4224)
    if delay:
        struct.pack_into("<I", data, 512, 1)
    encoded = library.encode("ascii") + b"\0"
    data[640:640 + len(encoded)] = encoded
    return data


class WorkerPeTests(unittest.TestCase):
    # Actual PE32/PE32+ and malformed format cases live in pe_imports_tests.rs
    # and execute through object. These tests own Python policy/CLI admission.
    def check(self, libraries, function=imported_libraries, architecture="x64", response=None):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bootstrap.exe"
            path.write_bytes(b"trusted build fixture")
            inspector = Path(directory) / "inspector.exe"
            inspector.write_bytes(b"inert controller fixture")
            value = dict(schema=1, architecture=architecture, imported_libraries=libraries, source="object_pe") if response is None else response
            result = subprocess.CompletedProcess([], 0, json.dumps(value), "")
            with patch("scripts.worker.pe.subprocess.run", return_value=result) as run:
                actual = function(path, architecture, inspector)
            self.assertEqual(run.call_args.args[0], [str(inspector), "inspect-pe", str(path), architecture])
            self.assertNotIn("shell", run.call_args.kwargs)
            self.assertEqual(run.call_args.kwargs["timeout"], 15)
            return actual

    def test_static_bootstrap_system_imports_are_accepted(self):
        for architecture in ["x64", "x86"]:
            self.assertEqual(self.check(["kernel32.dll"], require_static_crt, architecture), ["kernel32.dll"])

    def test_unsupported_architecture_never_launches_the_inspector(self):
        with patch("scripts.worker.pe.subprocess.run") as run, self.assertRaises(ValueError):
            imported_libraries("missing.exe", "arm64")
        run.assert_not_called()

    def test_object_observations_still_reject_external_crt_dependencies(self):
        for architecture in ["x64", "x86"]:
            for name in ["vcruntime140.dll", "msvcp140.dll", "msvcr120.dll", "ucrtbase.dll", "ucrtbased.dll", "api-ms-win-crt-runtime-l1-1-0.dll"]:
                with self.subTest(architecture=architecture, library=name), self.assertRaisesRegex(ValueError, "external C runtime"):
                    self.check([name], require_static_crt, architecture)

    def test_inspector_binding_and_library_schema_are_validated(self):
        for libraries in [[], ["../outside.dll"], ["KERNEL32.dll"], ["kernel32.dll", "kernel32.dll"], ["z.dll", "a.dll"], [1], "kernel32.dll"]:
            with self.subTest(libraries=libraries), self.assertRaises(ValueError):
                self.check(libraries)
        original = dict(schema=1, architecture="x64", imported_libraries=["kernel32.dll"], source="object_pe")
        for changed in [dict(schema=2), dict(architecture="x86"), dict(source="unverified"), dict(extra=True)]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.check([], response={**original, **changed})

    def test_failed_or_timed_out_inspection_has_no_parser_fallback(self):
        for error in [subprocess.CalledProcessError(1, "inspector"), subprocess.TimeoutExpired("inspector", 15)]:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "fixture.exe"
                path.write_bytes(b"inert")
                with patch("scripts.worker.pe.subprocess.run", side_effect=error), self.assertRaisesRegex(ValueError, "inspection failed"):
                    imported_libraries(path, inspector=path)


if __name__ == "__main__":
    unittest.main()
