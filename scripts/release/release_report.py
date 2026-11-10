"""Copy only compact package metadata for the Actions summary."""
import json
from pathlib import Path
import shutil


def create(source, destination):
    destination.mkdir(parents=True, exist_ok=True)
    for name in ("release.json", "validation-summary.json", "doctor.json", "SHA256SUMS"):
        path = source / name
        if path.is_file():
            shutil.copy2(path, destination / name)
    archives = [dict(name=path.name, size_bytes=path.stat().st_size) for path in sorted(source.glob("*.zip"))]
    (destination / "archives.json").write_text(json.dumps(archives, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    create(Path("dist"), Path("release-report"))
