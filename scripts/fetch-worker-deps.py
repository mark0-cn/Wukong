"""Fetch all pinned Docker build inputs on Linux or Windows (Python 3.11+)."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    manifest = json.loads((root / "workers/toolchain.lock.json").read_text())
    destination = root / "workers/deps"
    destination.mkdir(exist_ok=True)
    for name, entry in manifest.items():
        if not isinstance(entry, dict) or "url" not in entry:
            continue
        path = destination / entry["file"]
        if path.is_file() and not args.force:
            with path.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() == entry["sha256"]:
                    print(name + " cached and verified", flush=True)
                    continue
        temporary = path.with_suffix(path.suffix + ".download")
        subprocess.run(["curl", "--fail", "--location", "--retry", "5", "--retry-all-errors",
                        "--connect-timeout", "20", "--max-time", "1800", "--output", str(temporary), entry["url"]], check=True)
        with temporary.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != entry["sha256"]:
                raise ValueError(name + " checksum mismatch")
        temporary.replace(path)
        print(name + " downloaded and verified", flush=True)


if __name__ == "__main__":
    main()
