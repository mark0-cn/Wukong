import copy
import unittest
from release import validate_image, version


class ReleaseTests(unittest.TestCase):
    def test_repository_versions_agree(self):
        self.assertEqual(version("v" + version()), version())

    def test_mismatched_tag_is_rejected(self):
        with self.assertRaises(ValueError):
            version("v999.0.0")

    def test_image_identity_and_platform(self):
        data = dict(repository="hlmark0/re-agent-workbench", digest="sha256:" + "a" * 64,
                    image_id="sha256:" + "b" * 64, source_revision="abc", platform="linux/amd64", size_bytes=10)
        validate_image(data, "abc")
        for field, value in [("source_revision", "other"), ("platform", "linux/arm64"),
                             ("digest", "latest"), ("image_id", "wrong"),
                             ("repository", "other.example/x"), ("size_bytes", -1)]:
            changed = copy.copy(data)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_image(changed, "abc")


if __name__ == "__main__":
    unittest.main()
