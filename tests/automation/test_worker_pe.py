"""Bootstrap dependency validation rejects dynamic CRTs and malformed import tables."""
from pathlib import Path
import struct
import tempfile
import unittest

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
    def check(self, data, function=imported_libraries):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bootstrap.exe"
            path.write_bytes(data)
            return function(path)

    def test_static_bootstrap_system_imports_are_accepted(self):
        self.assertEqual(self.check(fixture("KERNEL32.dll"), require_static_crt), ["kernel32.dll"])

    def test_x86_self_test_imports_require_explicit_architecture(self):
        for delay in [False, True]:
            data = fixture("KERNEL32.dll", delay, "x86")
            with self.assertRaises(ValueError):
                self.check(data)
            self.assertEqual(self.check(data, lambda path: require_static_crt(path, "x86")), ["kernel32.dll"])
            with self.assertRaisesRegex(ValueError, "external C runtime"):
                self.check(fixture("VCRUNTIME140.dll", delay, "x86"), lambda path: require_static_crt(path, "x86"))

    def test_normal_and_delay_loaded_crt_dependencies_are_rejected(self):
        for delay in [False, True]:
            for name in ["VCRUNTIME140.dll", "MSVCP140.dll", "ucrtbase.dll", "ucrtbased.dll", "api-ms-win-crt-runtime-l1-1-0.dll"]:
                with self.subTest(delay=delay, library=name), self.assertRaisesRegex(ValueError, "external C runtime"):
                    self.check(fixture(name, delay), require_static_crt)

    def test_import_names_cannot_escape_a_library_directory(self):
        with self.assertRaisesRegex(ValueError, "library name"):
            self.check(fixture("../outside.dll"))

    def test_unmapped_names_and_truncated_images_are_rejected(self):
        data = fixture("kernel32.dll")
        struct.pack_into("<I", data, 524, 0xDEADBEEF)
        with self.assertRaises(ValueError):
            self.check(data)
        with self.assertRaises(ValueError):
            self.check(data[:170])

    def test_delay_imports_must_use_rva_addressing(self):
        data = fixture("kernel32.dll", True)
        struct.pack_into("<I", data, 512, 0)
        with self.assertRaisesRegex(ValueError, "delay-import addressing"):
            self.check(data)


if __name__ == "__main__":
    unittest.main()
