import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.release import verify_update

FIXTURES = Path(__file__).resolve().parents[1] / "fixtures/update-signature"
ROOT = Path(__file__).resolve().parents[2]
OPENSSL = shutil.which("openssl")


def manifest(**changes):
    value = {
        "schema": 1,
        "version": "0.2.0",
        "notes": "notes",
        "minimum_version": "0.1.2",
        "runtime_protocol": verify_update.RUNTIME_PROTOCOL,
        "data_schema": verify_update.DATA_SCHEMA,
        "image": "hlmark0/wukong@sha256:" + "a" * 64,
        "image_size": 123,
        "source_revision": "b" * 40,
        "platforms": {
            platform: {
                "url": f"{verify_update.RELEASES}/download/v0.2.0/Wukong-{platform}",
                "signature": "c2lnbmF0dXJl",
                "sha256": "d" * 64,
                "size": 7,
            }
            for platform in verify_update.PLATFORMS
        },
    }
    value.update(changes)
    return value


class ManifestTests(unittest.TestCase):
    def test_accepts_the_published_contract(self):
        verify_update.validate_manifest(manifest())

    def test_rejects_invalid_release_contracts(self):
        platform = verify_update.PLATFORMS[0]
        cases = [
            dict(version="0.2"),
            dict(version="0.2.0-rc.1"),
            dict(schema=2),
            dict(runtime_protocol=2),
            dict(data_schema=4),
            dict(source_revision="B" * 40),
            dict(image="hlmark0/other@sha256:" + "a" * 64),
            dict(image="hlmark0/wukong@sha256:" + "A" * 64),
            dict(image_size=0),
            dict(platforms={}),
            dict(platforms={platform: dict() for platform in verify_update.PLATFORMS}),
        ]
        for platform in verify_update.PLATFORMS:
            broken = manifest()
            broken["platforms"][platform]["url"] = "https://example.test/installer"
            cases.append(dict(platforms=broken["platforms"]))
            broken = manifest()
            broken["platforms"][platform]["url"] += "/../escape"
            cases.append(dict(platforms=broken["platforms"]))
            broken = manifest()
            broken["platforms"][platform]["sha256"] = "d" * 63
            cases.append(dict(platforms=broken["platforms"]))
            broken = manifest()
            broken["platforms"][platform]["size"] = 0
            cases.append(dict(platforms=broken["platforms"]))
            broken = manifest()
            broken["platforms"][platform]["signature"] = ""
            cases.append(dict(platforms=broken["platforms"]))
        for change in cases:
            with self.subTest(change=change):
                with self.assertRaises(verify_update.VerificationError):
                    verify_update.validate_manifest(manifest(**change))

    def test_checked_in_key_is_a_minisign_public_key(self):
        key_id, public = verify_update.public_key(
            (ROOT / "src/interfaces/desktop/update-public.key").read_text(encoding="utf-8")
        )
        self.assertEqual(len(key_id), 8)
        self.assertEqual(len(public), 32)


@unittest.skipUnless(OPENSSL, "requires the OpenSSL command line tool")
class SignatureTests(unittest.TestCase):
    def setUp(self):
        self.data = (FIXTURES / "message.txt").read_bytes()
        self.signature = (FIXTURES / "message.txt.sig").read_text(encoding="utf-8")
        self.public = (FIXTURES / "test.pub").read_text(encoding="utf-8")

    def test_accepts_the_real_minisign_signature(self):
        verify_update.verify_signature(self.data, self.signature, self.public)

    def test_rejects_modified_content_and_the_wrong_key(self):
        with self.assertRaises(verify_update.VerificationError):
            verify_update.verify_signature(self.data + b"changed", self.signature, self.public)
        with self.assertRaises(verify_update.VerificationError):
            verify_update.verify_signature(
                self.data,
                self.signature,
                (ROOT / "src/interfaces/desktop/update-public.key").read_text(encoding="utf-8"),
            )


class DirectoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        self.manifest = manifest()
        self.manifest["version"] = "0.2.0"
        self.manifest_bytes = json.dumps(self.manifest).encode()
        (self.directory / "latest.json").write_bytes(self.manifest_bytes)
        (self.directory / "latest.json.sig").write_text("signature", encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def test_rejects_missing_or_modified_artifacts(self):
        with patch.object(verify_update, "verify_signature"):
            with self.assertRaises(verify_update.VerificationError):
                verify_update.verify_directory(self.directory)
        for platform in verify_update.PLATFORMS:
            artifact = self.manifest["platforms"][platform]
            path = self.directory / artifact["url"].rsplit("/", 1)[1]
            path.write_bytes(b"installer")
            artifact["sha256"] = verify_update.sha256(path)
            artifact["size"] = path.stat().st_size
        (self.directory / "latest.json").write_bytes(json.dumps(self.manifest).encode())
        with patch.object(verify_update, "verify_signature"):
            verify_update.verify_directory(self.directory)
        (self.directory / self.manifest["platforms"]["linux-x86_64"]["url"].rsplit("/", 1)[1]).write_bytes(
            b"tampered"
        )
        with patch.object(verify_update, "verify_signature"):
            with self.assertRaises(verify_update.VerificationError):
                verify_update.verify_directory(self.directory)


if __name__ == "__main__":
    unittest.main()
