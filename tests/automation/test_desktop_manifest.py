import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts._lib.checksums import sha256
from scripts.release.desktop_manifest import generate

REVISION = "b" * 40
NUMBER = "0.2.0"
MEDIA_NAME = "wukong-windows-worker-media-" + NUMBER + ".zip"


class DesktopManifestTests(unittest.TestCase):
    def test_installer_and_runtime_share_the_trust_anchor(self):
        root = Path(__file__).resolve().parents[2]
        config = json.loads((root / "tauri.conf.json").read_text(encoding="utf-8"))
        self.assertEqual(
            config["plugins"]["updater"]["pubkey"],
            (root / "src/interfaces/desktop/update-public.key").read_text().strip(),
        )

    def validation(self, root):
        image = dict(
            repository="hlmark0/wukong",
            digest="sha256:" + "a" * 64,
            source_revision=REVISION,
            size_bytes=123,
        )
        (root / "image.json").write_text(json.dumps(image))
        report = dict(
            passed=True,
            source_revision=REVISION,
            image=image["repository"] + "@" + image["digest"],
        )
        (root / "validation-summary.json").write_text(json.dumps(report))
        content = root / "content.json"
        content.write_text(
            json.dumps(
                dict(
                    schema=1,
                    tag="v" + NUMBER,
                    version=NUMBER,
                    source_revision=REVISION,
                    entries=[],
                    changelog="Technical changes",
                )
            )
        )
        return content, report

    def installers(self, root):
        for name in ["Wukong_setup.exe", "Wukong.AppImage"]:
            (root / name).write_bytes(b"installer")
            (root / (name + ".sig")).write_text("signature")

    def accepted(self, root, *, media_name=MEDIA_NAME):
        content, _ = self.validation(root)
        self.installers(root)
        if media_name is not None:
            (root / media_name).write_bytes(b"worker template")
        return content

    def build(self, root, content):
        with (
            patch(
                "scripts.release.desktop_manifest.subprocess.check_output",
                return_value=REVISION,
            ),
            patch("scripts.release.desktop_manifest.version", return_value=NUMBER),
            patch("scripts.release.desktop_manifest.subprocess.run") as sign,
        ):
            manifest = generate(root, root, content)
        sign.assert_called_once()
        return manifest

    def test_requires_both_platforms_and_matching_acceptance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            content, report = self.validation(root)
            with (
                patch(
                    "scripts.release.desktop_manifest.subprocess.check_output",
                    return_value=REVISION,
                ),
                patch("scripts.release.desktop_manifest.version", return_value=NUMBER),
                patch("scripts.release.desktop_manifest.subprocess.run") as sign,
            ):
                with self.assertRaises(ValueError):
                    generate(root, root, content)
                self.installers(root)
                manifest = generate(root, root, content)
                self.assertEqual(set(manifest["platforms"]), {"windows-x86_64", "linux-x86_64"})
                self.assertEqual(manifest["image"], report["image"])
                self.assertIn("release_content_sha256", manifest)
                self.assertEqual((root / "release-content.json").read_bytes(), content.read_bytes())
                sign.assert_called_once()
                report["source_revision"] = "c" * 40
                (root / "validation-summary.json").write_text(json.dumps(report))
                with self.assertRaises(ValueError):
                    generate(root, root, content)

    def test_worker_media_is_signed_into_both_release_catalogs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            content = self.accepted(root)
            manifest = self.build(root, content)
            media = root / MEDIA_NAME
            expected = dict(
                version=NUMBER,
                name=MEDIA_NAME,
                size=media.stat().st_size,
                sha256=sha256(media),
                url="https://github.com/mark0-cn/Wukong/releases/download/v"
                + NUMBER
                + "/"
                + MEDIA_NAME,
            )
            self.assertEqual(manifest["worker_media"], expected)
            self.assertEqual(
                json.loads((root / "latest.json").read_text(encoding="utf-8")), manifest
            )
            release = json.loads((root / "release.json").read_text(encoding="utf-8"))
            self.assertEqual(release["worker_media"], expected)
            self.assertEqual(release["platforms"], ["windows-x86_64", "linux-x86_64"])
            # The optional template is a separate asset, never a platform update entry.
            self.assertNotIn(MEDIA_NAME, json.dumps(manifest["platforms"]))
            self.assertTrue(media.is_file())
            first = (root / "latest.json").read_bytes()
            self.build(root, content)
            self.assertEqual((root / "latest.json").read_bytes(), first)

    def test_rehearsal_without_worker_media_keeps_the_existing_catalog(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            content = self.accepted(root, media_name=None)
            manifest = self.build(root, content)
            self.assertEqual(
                set(manifest),
                {
                    "schema",
                    "version",
                    "notes",
                    "minimum_version",
                    "release_content_sha256",
                    "runtime_protocol",
                    "data_schema",
                    "image",
                    "image_size",
                    "source_revision",
                    "platforms",
                },
            )
            release = json.loads((root / "release.json").read_text(encoding="utf-8"))
            self.assertNotIn("worker_media", release)
            self.assertEqual(set(release["platforms"]), set(manifest["platforms"]))

    def test_stale_worker_media_asset_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            content = self.accepted(root, media_name="wukong-windows-worker-media-9.9.9.zip")
            with self.assertRaisesRegex(ValueError, "Worker media"):
                self.build(root, content)


if __name__ == "__main__":
    unittest.main()
