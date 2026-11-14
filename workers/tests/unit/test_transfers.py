"""Real file transfer and ledger recovery; no analysed program is executed."""
import base64
import hashlib
from pathlib import Path
import tempfile
import unittest

from worker_runtime import Runtime
from worker_transfers import CHUNK_BYTES, MAX_TRANSFER_BYTES


class TransferTests(unittest.TestCase):
    def begin(self, runtime, content, path="sample.bin"):
        return runtime.dispatch("file.upload.begin", {
            "path": path, "size": len(content), "sha256": hashlib.sha256(content).hexdigest(),
            "request_id": "begin-" + path,
        })["transfer_id"]

    def write(self, runtime, transfer, offset, content, request="write"):
        return runtime.dispatch("file.upload.write", {
            "transfer_id": transfer, "offset": offset, "data_base64": base64.b64encode(content).decode(),
            "request_id": request,
        })

    def test_resume_after_daemon_restart_and_publish_verified_immutable_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = Runtime(root, trusted=True)
            content = b"first chunk\0second chunk"
            transfer = self.begin(runtime, content)
            self.write(runtime, transfer, 0, content[:12], "first")
            runtime = Runtime(root, trusted=True)
            self.assertEqual(self.begin(runtime, content), transfer)
            self.assertEqual(runtime.dispatch("file.upload.status", {"transfer_id": transfer})["offset"], 12)
            # Retrying an acknowledged operation does not append the bytes again.
            self.write(runtime, transfer, 0, content[:12], "first")
            self.write(runtime, transfer, 12, content[12:], "second")
            args = {"transfer_id": transfer, "request_id": "commit"}
            artifact = runtime.dispatch("file.upload.commit", args)
            self.assertEqual(runtime.dispatch("file.upload.commit", args), artifact)
            self.assertEqual(artifact["sha256"], hashlib.sha256(content).hexdigest())
            self.assertEqual((root / artifact["path"]).read_bytes(), content)
            self.assertTrue(runtime.dispatch("file.upload.status", {"transfer_id": transfer})["complete"])

    def test_large_file_exceeds_old_limit_without_oversized_rpc_messages(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            chunk = b"A" * CHUNK_BYTES
            digest = hashlib.sha256()
            for _ in range(33):
                digest.update(chunk)
            size = 33 * CHUNK_BYTES
            transfer = runtime.dispatch("file.upload.begin", {
                "path": "large.dmp", "size": size, "sha256": digest.hexdigest(),
            })["transfer_id"]
            for index in range(33):
                result = self.write(runtime, transfer, index * CHUNK_BYTES, chunk, "chunk-" + str(index))
                self.assertEqual(result["offset"], (index + 1) * CHUNK_BYTES)
            result = runtime.dispatch("file.upload.commit", {"transfer_id": transfer})
            self.assertEqual(result["size"], size)
            self.assertEqual(result["sha256"], digest.hexdigest())
            read = runtime.dispatch("file.read", {"path": result["path"], "offset": size - 17, "max_bytes": 64})
            self.assertEqual(base64.b64decode(read["data_base64"]), b"A" * 17)
            self.assertEqual(read["chunk_sha256"], hashlib.sha256(b"A" * 17).hexdigest())

    def test_overlap_mismatch_gaps_hash_failure_and_private_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            transfer = self.begin(runtime, b"abcdef")
            self.write(runtime, transfer, 0, b"abc", "first")
            with self.assertRaisesRegex(ValueError, "different content"):
                self.write(runtime, transfer, 0, b"wrong", "wrong")
            with self.assertRaisesRegex(ValueError, "contiguous"):
                self.write(runtime, transfer, 5, b"f", "gap")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                runtime.dispatch("file.upload.commit", {"transfer_id": transfer})
            self.write(runtime, transfer, 3, b"xyz", "bad-tail")
            with self.assertRaisesRegex(ValueError, "SHA256"):
                runtime.dispatch("file.upload.commit", {"transfer_id": transfer})
            for path in ["../outside", "../../outside", "x:stream", "a\\..\\outside"]:
                with self.assertRaises(ValueError):
                    self.begin(runtime, b"x", path)
            for path in ["transfers/" + transfer + "/manifest.json", "../outside", "operations/private"]:
                with self.assertRaises(ValueError):
                    runtime.dispatch("file.read", {"path": path, "offset": 0})
            with self.assertRaises(ValueError):
                runtime.dispatch("file.upload.begin", {"path":"large", "size":MAX_TRANSFER_BYTES + 1, "sha256":"0"*64})

    def test_partial_unacknowledged_write_is_completed_without_repeating_its_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            transfer = self.begin(runtime, b"abcdef")
            # Simulate a crash partway through an unacknowledged chunk.
            (Path(directory) / "transfers" / transfer / "data").write_bytes(b"ab")
            result = self.write(runtime, transfer, 0, b"abcdef")
            self.assertEqual(result["offset"], 6)
            runtime.dispatch("file.upload.commit", {"transfer_id":transfer})
            self.assertEqual((Path(directory) / "files/sample.bin").read_bytes(), b"abcdef")

    def test_empty_file_and_existing_destination_conflict(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            transfer = self.begin(runtime, b"")
            result = runtime.dispatch("file.upload.commit", {"transfer_id":transfer})
            self.assertEqual(result["size"], 0)
            with self.assertRaisesRegex(ValueError, "different content"):
                runtime.dispatch("file.upload.begin", {"path":"sample.bin", "size":1, "sha256":hashlib.sha256(b"x").hexdigest()})
