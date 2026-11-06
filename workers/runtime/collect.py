#!/usr/bin/env python3
"""Bounded artifact collector, run under one experiment's unprivileged UID."""
import base64
import json
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
mode = sys.argv[2]
files, stats, total = [], {}, 0
if root.is_dir():
    if mode == "fuzz":
        for path in sorted(root.glob("*/fuzzer_stats"))[:32]:
            if not path.is_symlink() and root in path.resolve().parents:
                text = path.read_text(errors="replace")[:65536]
                stats[path.parent.name] = dict((line.split(":", 1)[0].strip(), line.split(":", 1)[1].strip()) for line in text.splitlines() if ":" in line)
        candidates = sorted(root.glob("*/crashes/id*"))[:128]
    else:
        candidates = sorted(root.rglob("*"))[:256]
    for path in candidates:
        if not path.is_file() or path.is_symlink() or root not in path.resolve().parents:
            continue
        size = path.stat().st_size
        if size > 1048576 or total + size > 16 * 1048576:
            continue
        data = path.read_bytes()
        if len(data) > 1048576:
            continue
        total += len(data)
        files.append({"path": path.relative_to(root).as_posix(), "data": base64.b64encode(data).decode()})
print(json.dumps({"stats": stats, "files": files}))
