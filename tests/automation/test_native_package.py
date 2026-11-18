from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from scripts.release import native_package


class NativePackageTests(unittest.TestCase):
    def test_existing_package_output_is_replaced(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Windows"),
            patch.object(native_package.platform, "machine", return_value="AMD64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="C:/tool.exe"),
            patch.object(native_package, "build"),
            patch.object(native_package.worker_setup, "ensure_writable_temporary"),
        ):
            root = Path(tmp)
            built = root / "bundle/nsis/Wukong_1.0.0_x64-setup.exe"
            built.parent.mkdir(parents=True)
            built.write_bytes(b"new installer")
            output = root / "output"
            output.mkdir()
            (output / "Wukong_0.9.0_x64-setup.exe").write_bytes(b"old installer")
            with patch.object(native_package.Path, "glob", return_value=[built]):
                native_package.package(output)
            self.assertEqual((output / built.name).read_bytes(), b"new installer")
            self.assertFalse((output / "Wukong_0.9.0_x64-setup.exe").exists())

    def test_unrelated_output_content_is_never_deleted(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Windows"),
            patch.object(native_package.platform, "machine", return_value="AMD64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="C:/tool.exe"),
            patch.object(native_package, "build") as build,
            patch.object(native_package.worker_setup, "ensure_writable_temporary"),
        ):
            output = Path(tmp) / "output"
            output.mkdir()
            (output / "notes.txt").write_text("keep", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not created by package-local"):
                native_package.package(output)
            build.assert_not_called()
            self.assertEqual((output / "notes.txt").read_text(encoding="utf-8"), "keep")

    def test_preflight_never_builds_or_fetches_images(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Linux"),
            patch.object(native_package.platform, "machine", return_value="x86_64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="/bin/tool"),
            patch.object(native_package, "build") as build,
        ):
            output = Path(tmp) / "output"
            native_package.package(output, check=True)
            build.assert_not_called()
            self.assertFalse(output.exists())

    def test_windows_preflight_checks_the_worker_setup_without_building(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Windows"),
            patch.object(native_package.platform, "machine", return_value="AMD64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="C:/tool.exe"),
            patch.object(
                native_package.worker_setup,
                "check",
                return_value={"toolchain": {"available": False}},
            ) as check,
            patch.object(native_package, "build") as build,
        ):
            output = Path(tmp) / "output"
            native_package.package(output, check=True)
            check.assert_called_once_with()
            build.assert_not_called()
            self.assertFalse(output.exists())

    def test_package_local_defaults_to_the_local_release_build(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Linux"),
            patch.object(native_package.platform, "machine", return_value="x86_64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="/bin/tool"),
            patch.object(native_package, "build") as build,
            patch.object(native_package.Path, "glob", return_value=[]),
            patch.object(native_package.worker_setup, "ensure_writable_temporary"),
        ):
            with self.assertRaisesRegex(ValueError, "Expected one installer"):
                native_package.package(Path(tmp) / "output")
        self.assertTrue(build.call_args.kwargs["local_release"])

    def test_online_option_restores_the_catalog_build(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Linux"),
            patch.object(native_package.platform, "machine", return_value="x86_64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="/bin/tool"),
            patch.object(native_package, "build") as build,
            patch.object(native_package.Path, "glob", return_value=[]),
            patch.object(native_package.worker_setup, "ensure_writable_temporary"),
        ):
            with self.assertRaisesRegex(ValueError, "Expected one installer"):
                native_package.package(Path(tmp) / "output", local_release=False)
        self.assertFalse(build.call_args.kwargs["local_release"])


class WritableTemporaryNativePackageTests(unittest.TestCase):
    def test_windows_package_prepares_a_writable_temporary(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(native_package.platform, "system", return_value="Windows"),
            patch.object(native_package.platform, "machine", return_value="AMD64"),
            patch.object(native_package, "checkout_identity", return_value="a" * 40),
            patch.object(native_package.shutil, "which", return_value="C:/tool.exe"),
            patch.object(native_package, "build"),
            patch.object(native_package.Path, "glob", return_value=[]),
            patch.object(native_package.worker_setup, "ensure_writable_temporary") as temporary,
        ):
            with self.assertRaisesRegex(ValueError, "Expected one installer"):
                native_package.package(Path(tmp) / "output")
        temporary.assert_called_once()


if __name__ == "__main__":
    unittest.main()
