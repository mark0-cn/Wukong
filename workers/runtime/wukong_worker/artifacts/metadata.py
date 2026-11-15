"""Atomic metadata publication and bounded sharing-conflict retries."""

import json
import os
from pathlib import Path
import time
from typing import Any
from atomicwrites import AtomicWriter, atomic_write


class _MetadataWriter(AtomicWriter):
    def commit(self, stream):
        # Preserve the bounded Windows sharing retry. The library owns temporary
        # files, fsync, write-through replacement and rollback on every failure.
        for attempt in range(5):
            try:
                return super().commit(stream)
            except OSError as error:
                if (
                    os.name != "nt"
                    or getattr(error, "winerror", None) not in {5, 32, 33}
                    or attempt == 4
                ):
                    raise
                time.sleep(0.02 * (attempt + 1))


def atomic_json(path: str | Path, data: Any) -> None:
    encoded = json.dumps(data, ensure_ascii=False)
    with atomic_write(path, writer_cls=_MetadataWriter, overwrite=True, encoding="utf-8") as stream:
        stream.write(encoded)


def load_json(path: str | Path) -> Any:
    # Windows: a concurrent atomic replacement can fail the open with a
    # transient sharing violation; retry briefly like atomic_json does.
    path = Path(path)
    for attempt in range(5):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except OSError:
            if attempt == 4:
                raise
            time.sleep(0.02 * (attempt + 1))
