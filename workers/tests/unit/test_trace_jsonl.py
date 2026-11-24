import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from wukong_worker.artifacts.jsonl import BoundedJsonlWriter


class JsonlWriterTests(unittest.TestCase):
    def test_limit_preserves_complete_prefix_and_hash(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            writer = BoundedJsonlWriter(path, 64)
            self.assertTrue(writer.write({"sequence": 0, "value": "first"}))
            self.assertFalse(writer.write({"sequence": 1, "value": "x" * 100}))
            result = writer.close()
            rows = path.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(rows), 1)
            self.assertEqual(json.loads(rows[0])["sequence"], 0)
            self.assertEqual(result["events_written"], 1)
            self.assertTrue(result["jsonl_limited"])
            self.assertEqual(result["jsonl_bytes"], len(rows[0].encode()) + 1)

    def test_zero_length_file_is_valid_empty_prefix(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            writer = BoundedJsonlWriter(path, 1)
            self.assertFalse(writer.write({"event": "does-not-fit"}))
            result = writer.close()
            self.assertEqual(path.read_bytes(), b"")
            self.assertEqual(result["events_written"], 0)
            self.assertEqual(result["jsonl_bytes"], 0)


if __name__ == "__main__":
    unittest.main()
