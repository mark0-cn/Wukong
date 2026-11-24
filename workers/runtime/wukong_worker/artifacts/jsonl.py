"""Bounded JSON Lines publication with a complete, hashable prefix."""

import hashlib
import json
import os
from pathlib import Path


class BoundedJsonlWriter:
    def __init__(self, path, max_bytes):
        self.path = Path(path)
        self.max_bytes = int(max_bytes)
        self.bytes_written = 0
        self.records_written = 0
        self.limited = False
        self.closed = False
        self._digest = hashlib.sha256()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._stream = self.path.open("xb")

    def write(self, record):
        if self.closed:
            raise ValueError("JSONL writer is closed")
        if self.limited:
            return False
        encoded = (
            json.dumps(record, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n"
        ).encode("utf-8")
        if len(encoded) > self.max_bytes or self.bytes_written + len(encoded) > self.max_bytes:
            self.limited = True
            return False
        before = self.bytes_written
        try:
            self._stream.write(encoded)
            self._stream.flush()
        except BaseException:
            self._stream.seek(before)
            self._stream.truncate(before)
            self._stream.flush()
            raise
        self._digest.update(encoded)
        self.bytes_written += len(encoded)
        self.records_written += 1
        return True

    def close(self):
        if not self.closed:
            self._stream.flush()
            os.fsync(self._stream.fileno())
            self._stream.close()
            self.closed = True
        return {
            "jsonl_bytes": self.bytes_written,
            "jsonl_sha256": self._digest.hexdigest(),
            "events_written": self.records_written,
            "jsonl_limited": self.limited,
        }
