"""Bind reusable installers to source, packaging recipe, signing key and exact bytes.

Only artifacts from a successful trusted installer job may reach this verifier.
The final publish gate additionally verifies actual updater signatures with Rust.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
from scripts._lib.versioning import ROOT, version
from scripts._lib.checksums import sha256


def identity(target, root=ROOT):
    if target not in ("windows", "linux"):
        raise ValueError("Unsupported installer platform")
    recipe = hashlib.sha256()
    for name in (".github/workflows/build-installer.yml", "scripts/release/installer_artifact.py",
                 "scripts/release/installer_identity.py", "tauri.conf.json", "tauri." + target + ".conf.json",
                 "rust-toolchain.toml", "package-lock.json", "src/desktop/update-public.key"):
        recipe.update(name.encode() + b"\0" + (root / name).read_bytes())
    return dict(schema=1, target=target, recipe=recipe.hexdigest(), version=version(root=root),
                source_revision=subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip())


def record(target, directory, root=ROOT):
    pattern = "*-setup.exe" if target == "windows" else "*.AppImage"
    candidates = list(directory.glob(pattern))
    if len(candidates) != 1:
        raise ValueError("Expected exactly one installer")
    binary = candidates[0]
    files = [binary, binary.with_name(binary.name + ".sig")]
    entries = {}
    for path in files:
        if path.is_symlink() or not path.is_file() or not path.stat().st_size:
            raise ValueError("Missing or invalid installer/signature")
        entries[path.name] = dict(size=path.stat().st_size, sha256=sha256(path))
    manifest = dict(identity(target, root), files=entries)
    (directory / ("installer-" + target + ".json")).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def verify(target, directory, root=ROOT):
    metadata = directory / ("installer-" + target + ".json")
    if metadata.is_symlink():
        raise ValueError("Invalid installer manifest")
    manifest = json.loads(metadata.read_text(encoding="utf-8"))
    for key, value in identity(target, root).items():
        if manifest.get(key) != value:
            raise ValueError("Installer identity mismatch: " + key)
    files = manifest.get("files", {})
    suffix = "-setup.exe" if target == "windows" else ".AppImage"
    binaries = [name for name in files if name.endswith(suffix)]
    if len(binaries) != 1 or set(files) != {binaries[0], binaries[0] + ".sig"}:
        raise ValueError("Unexpected installer asset set")
    if {p.name for p in directory.iterdir()} != set(files) | {metadata.name}:
        raise ValueError("Unexpected files in installer artifact")
    for name, expected in files.items():
        if Path(name).name != name or "/" in name or "\\" in name:
            raise ValueError("Unsafe installer filename")
        path = directory / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size != expected.get("size") or sha256(path) != expected.get("sha256"):
            raise ValueError("Installer checksum/size mismatch: " + name)
    print("Verified reusable", target, "installer at", manifest["source_revision"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=["windows", "linux"], required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    verify(args.target, args.directory)
