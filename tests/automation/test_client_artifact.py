import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import scripts.release.client_artifact as client


class ClientTests(unittest.TestCase):
    def test_identity_and_bytes_must_both_match(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / client.TARGETS["windows"][1]
            source.parent.mkdir(parents=True)
            source.write_bytes(b"client")
            folder = root / "artifact"
            expected = dict(schema=1, target="windows", recipe="recipe", version="1.2.3", source_revision="a" * 40)
            with patch.object(client, "identity", return_value=expected):
                client.stage("windows", folder, root)
                client.verify("windows", folder, root)
                manifest_path = folder / "client.json"
                original = json.loads(manifest_path.read_text())
                for key in (*expected, "filename", "sha256", "size"):
                    broken = dict(original, **{key: "wrong"})
                    manifest_path.write_text(json.dumps(broken))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        client.verify("windows", folder, root)
                manifest_path.write_text(json.dumps(original))
                (folder / "wukong.exe").write_bytes(b"tamper")
                with self.assertRaises(ValueError):
                    client.verify("windows", folder, root)

    def test_desktop_and_acceptance_profiles_differ(self):
        self.assertNotEqual(client.TARGETS["linux"][0], client.TARGETS["linux-desktop"][0])
        self.assertIn("ubuntu-22.04", client.TARGETS["linux-desktop"][2])
        self.assertNotEqual(client.identity("linux")["profile"], client.identity("linux-desktop")["profile"])
