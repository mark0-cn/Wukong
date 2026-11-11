import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from scripts.release import installer_identity as installer


class InstallerIdentityTests(unittest.TestCase):
    def test_tampered_bytes_signature_recipe_and_source_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            binary = folder / "Wukong_1.2.3_x64-setup.exe"
            signature = folder / (binary.name + ".sig")
            binary.write_bytes(b"installer")
            signature.write_bytes(b"signature")
            identity = dict(schema=1, target="windows", recipe="recipe", version="1.2.3", source_revision="a" * 40)
            with patch.object(installer, "identity", return_value=identity):
                installer.record("windows", folder)
                installer.verify("windows", folder)
                metadata = folder / "installer-windows.json"
                original = metadata.read_text()
                for key in identity:
                    value = json.loads(original)
                    value[key] = "wrong"
                    metadata.write_text(json.dumps(value))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        installer.verify("windows", folder)
                metadata.write_text(original)
                for path in (binary, signature):
                    content = path.read_bytes()
                    path.write_bytes(b"tampered")
                    with self.assertRaises(ValueError):
                        installer.verify("windows", folder)
                    path.write_bytes(content)
                (folder / "extra.exe").write_bytes(b"unexpected")
                with self.assertRaises(ValueError):
                    installer.verify("windows", folder)

    def test_current_signing_config_is_in_identity(self):
        self.assertEqual(installer.identity("linux")["target"], "linux")
        self.assertNotEqual(installer.identity("windows")["recipe"], installer.identity("linux")["recipe"])
