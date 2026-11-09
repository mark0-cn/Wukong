"""Fetch release-pinned portable tools. Run on the build host, not in a target worker."""
from pathlib import Path
import hashlib
import json
import subprocess

root = Path(__file__).resolve().parents[1]
manifest = json.loads((root / "workers/toolchain.lock.json").read_text())
destination = root / "workers/deps"
destination.mkdir(exist_ok=True)
for name in ["pwndbg", "pwninit"]:
    entry = manifest[name]
    path = destination / entry["file"]
    cached = None
    if path.is_file():
        with path.open("rb") as stream: cached = hashlib.file_digest(stream,"sha256").hexdigest()
    if cached == entry["sha256"]:
        print(name + " cached and verified", flush=True)
        continue
    temporary = path.with_suffix(path.suffix + ".download")
    subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--connect-timeout", "20",
                    "--output", str(temporary), entry["url"]], check=True)
    with temporary.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != entry["sha256"]:
            raise ValueError(name + " checksum mismatch")
    temporary.replace(path)
    print(name + " downloaded and verified", flush=True)
