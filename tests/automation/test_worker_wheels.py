"""Real offline pip resolution; no fixture is installed or executed."""
from pathlib import Path
import os
import tempfile
import unittest
import zipfile
from unittest.mock import patch

from scripts.worker.python_wheels import verify


def wheel(root, name, dependency="", requires_python=">=3.10"):
    archive = root / f"{name}-1.0-py3-none-any.whl"
    with zipfile.ZipFile(archive, "w") as output:
        prefix = f"{name}-1.0.dist-info/"
        output.writestr(prefix + "WHEEL", "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n")
        output.writestr(prefix + "METADATA", f"Metadata-Version: 2.1\nName: {name}\nVersion: 1.0\nRequires-Python: {requires_python}\n" +
                        (f"Requires-Dist: {dependency}\n" if dependency else ""))
        output.writestr(prefix + "RECORD", "")
        output.writestr(name + ".py", "raise RuntimeError('fixture must not execute')")
    return archive


class WheelResolutionTests(unittest.TestCase):
    def test_exact_transitive_wheels_resolve_without_installation_or_network(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            required = root / "requirements.txt"
            required.write_text("fixture==1.0\ndependency==1.0\n")
            archives = [wheel(root, "fixture", "dependency>=1"), wheel(root, "dependency")]
            self.assertEqual(verify(archives, required, "3.13.17"),
                             {"wheels": 2, "dependency_closure": True, "network": False})
            self.assertEqual(len(list(root.iterdir())), 3)

    def test_missing_version_conflicting_or_conditional_dependencies_are_rejected(self):
        for dependency in ["missing-runtime", "dependency>=2", "dependency; sys_platform == 'win32'",
                           "dependency @ https://example.invalid/dependency.whl"]:
            with self.subTest(dependency=dependency), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                required = root / "requirements.txt"
                required.write_text("fixture==1.0\ndependency==1.0\n")
                with self.assertRaises(ValueError):
                    verify([wheel(root, "fixture", dependency), wheel(root, "dependency")], required, "3.13.17")

    def test_target_python_requirement_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            required = root / "requirements.txt"
            required.write_text("fixture==1.0\n")
            with self.assertRaisesRegex(ValueError, "dependency closure"):
                verify([wheel(root, "fixture", requires_python=">=4")], required, "3.13.17")

    def test_requirements_cannot_enable_remote_indexes_or_direct_references(self):
        for line in ["--find-links https://example.invalid/", "fixture @ https://example.invalid/f.whl"]:
            with self.subTest(line=line), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                required = root / "requirements.txt"
                required.write_text(line)
                with self.assertRaisesRegex(ValueError, "plain version pins"):
                    verify([wheel(root, "fixture")], required, "3.13.17")

    def test_controller_pip_configuration_is_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "pip.ini"
            config.write_text("[global]\ninvalid ini configuration\n")
            required = root / "requirements.txt"
            required.write_text("fixture==1.0\n")
            with patch.dict(os.environ, {"PIP_CONFIG_FILE": str(config)}):
                self.assertTrue(verify([wheel(root, "fixture")], required, "3.13.17")["dependency_closure"])
