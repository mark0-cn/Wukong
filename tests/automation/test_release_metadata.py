import hashlib
import io
import json
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from scripts.release import release_metadata


class ReleaseMetadataTests(unittest.TestCase):
    def setUp(self):
        self.revision = "a" * 40
        self.manifest = dict(schema_version=2, version="1.2.3", source_revision=self.revision,
                             platform="windows-amd64", tool_platform="linux/amd64",
                             image="example/wukong@sha256:" + "b" * 64,
                             image_id="sha256:" + "c" * 64, image_size_bytes=12345,
                             runtime_protocol=3, data_schema=5)

    def assets(self, mutate_sums=lambda text: text):
        manifest = json.dumps(self.manifest).encode()
        sums = mutate_sums(hashlib.sha256(manifest).hexdigest() + "  release.json\n").encode()
        return [manifest, sums]

    def test_exact_version_release_is_converted_to_image_identity(self):
        with patch.object(release_metadata, "download", side_effect=self.assets()) as download:
            image = release_metadata.release_image("1.2.3", self.revision)
        base = "https://github.com/mark0-cn/Wukong/releases/download/v1.2.3/"
        self.assertEqual([call.args[0] for call in download.call_args_list],
                         [base + "release.json", base + "SHA256SUMS"])
        self.assertEqual(image, dict(repository="example/wukong", digest="sha256:" + "b" * 64,
                                    image_id=self.manifest["image_id"], source_revision=self.revision,
                                    size_bytes=12345, platform="linux/amd64", release_version="1.2.3"))

    def test_earlier_image_commit_and_legacy_repository_are_preserved(self):
        old = "d" * 40
        self.manifest["source_revision"] = old
        self.manifest["image"] = self.manifest["image"].replace("/wukong@", "/re-agent-workbench@")
        with patch.object(release_metadata, "download", side_effect=self.assets()):
            image = release_metadata.release_image("1.2.3", self.revision)
        self.assertEqual(image["source_revision"], old)
        self.assertEqual(image["repository"], "example/re-agent-workbench")
        self.assertEqual(image["release_version"], "1.2.3")

    def test_separate_image_source_takes_precedence_over_client_source(self):
        self.manifest["image_source_revision"] = "e" * 40
        with patch.object(release_metadata, "download", side_effect=self.assets()):
            image = release_metadata.release_image("1.2.3", self.revision)
        self.assertEqual(image["source_revision"], "e" * 40)

    def test_bad_missing_and_duplicate_checksums_are_rejected(self):
        for mutate in (lambda text: "0" * 64 + "  release.json\n",
                       lambda text: text.replace("release.json", "other.json"),
                       lambda text: text + text):
            with self.subTest(mutate=mutate), patch.object(release_metadata, "download", side_effect=self.assets(mutate)):
                with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
                    release_metadata.release_image("1.2.3", self.revision)

    def test_incompatible_release_is_rejected_even_with_valid_checksums(self):
        original = dict(self.manifest)
        for field, value in (("version", "1.2.4"), ("source_revision", "invalid"), ("schema_version", 1),
                             ("platform", "linux-amd64"), ("tool_platform", "linux/arm64"),
                             ("runtime_protocol", 1), ("data_schema", 1), ("image", "example/wukong:latest"),
                             ("image_id", None), ("image_size_bytes", -1)):
            self.manifest = {**original, field: value}
            with self.subTest(field=field), patch.object(release_metadata, "download", side_effect=self.assets()):
                with self.assertRaises(ValueError):
                    release_metadata.release_image("1.2.3", self.revision)

    def test_repository_is_validated_before_any_request(self):
        with patch.object(release_metadata, "download") as download:
            for repository in ("https://example.com", "owner/../repo", "owner/repo?token=x"):
                with self.subTest(repository=repository), self.assertRaises(ValueError):
                    release_metadata.release_image("1.2.3", self.revision, repository)
        download.assert_not_called()

    def test_missing_release_does_not_fall_back_to_latest(self):
        error = HTTPError("https://github.com/asset", 404, "Not found", {}, None)
        with patch.object(release_metadata.urllib.request, "build_opener") as opener:
            opener.return_value.open.side_effect = error
            with self.assertRaisesRegex(ValueError, "Publish this version first"):
                release_metadata.release_image("1.2.3", self.revision)
            opener.return_value.open.assert_called_once()
            request = opener.return_value.open.call_args.args[0]
            self.assertIn("/releases/download/v1.2.3/release.json", request.full_url)

    def test_oversized_download_is_rejected(self):
        with patch.object(release_metadata.urllib.request, "build_opener") as opener:
            opener.return_value.open.return_value = io.BytesIO(b"x" * (release_metadata.MAX_METADATA_BYTES + 1))
            with self.assertRaisesRegex(ValueError, "exceeds"):
                release_metadata.download("https://github.com/asset")

    def test_insecure_redirect_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "HTTPS"):
            release_metadata.HttpsRedirect().redirect_request(None, None, 302, "Found", {}, "http://example.com/asset")


if __name__ == "__main__":
    unittest.main()
