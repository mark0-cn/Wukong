"""Fetch all pinned Docker build inputs on Linux or Windows (Python 3.11+)."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
from concurrent.futures import ThreadPoolExecutor
import re


def entries(manifest):
    files, result = set(), []
    for name, entry in manifest.items():
        if not isinstance(entry, dict) or "url" not in entry:
            continue
        filename = entry.get("file", "")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", filename) or filename.lower() in files:
            raise ValueError("Unsafe or duplicate dependency filename: " + filename)
        if not re.fullmatch(r"[0-9a-f]{64}", entry.get("sha256", "")) or not entry["url"].startswith("https://"):
            raise ValueError("Dependency requires HTTPS and SHA256: " + name)
        files.add(filename.lower())
        result.append((name, entry))
    return result


def fetch(item, destination, force=False):
    name, entry = item
    path = destination / entry["file"]
    if path.is_file() and not force:
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() == entry["sha256"]:
                print(name + " cached and verified", flush=True)
                return
    temporary = path.with_suffix(path.suffix + ".download")
    try:
        subprocess.run(["curl", "--fail", "--location", "--silent", "--show-error", "--retry", "5", "--retry-all-errors",
                        "--connect-timeout", "20", "--max-time", "1800", "--output", str(temporary), entry["url"]], check=True)
        with temporary.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != entry["sha256"]:
                raise ValueError(name + " checksum mismatch")
        temporary.replace(path)
        print(name + " downloaded and verified", flush=True)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--jobs", type=int, choices=range(1, 5), default=3, help="Concurrent downloads (1-4)")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    manifest = json.loads((root / "workers/toolchain.lock.json").read_text())
    destination = root / "workers/deps"
    destination.mkdir(exist_ok=True)
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(lambda item: fetch(item, destination, args.force), entries(manifest)))


if __name__ == "__main__":
    main()
