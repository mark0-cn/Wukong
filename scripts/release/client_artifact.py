"""Create and verify exact-source client artifacts before executing or packaging them."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
from scripts._lib.versioning import ROOT, version
from scripts._lib.checksums import sha256 as digest

TARGETS = {
    "windows": ("windows-client-v1", "target/release/wukong.exe", "release/custom-protocol/windows-amd64"),
    "linux": ("linux-acceptance-client-v1", "target/debug/wukong", "dev/debug=1/no-default-features/linux-amd64"),
}


def identity(target, root=ROOT):
    recipe = hashlib.sha256()
    for name in (".github/workflows/build-client.yml", "scripts/release/client_artifact.py", "rust-toolchain.toml"):
        recipe.update(name.encode() + b"\0" + (root / name).read_bytes())
    return dict(schema=1, target=target, recipe=recipe.hexdigest(),
                profile=TARGETS[target][2], version=version(root=root),
                source_revision=subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"]).decode().strip())


def stage(target, directory, root=ROOT):
    source = root / TARGETS[target][1]
    directory.mkdir(parents=True, exist_ok=True)
    output = directory / source.name
    shutil.copy2(source, output)
    manifest = dict(identity(target, root), filename=source.name, sha256=digest(output), size=output.stat().st_size)
    (directory / "client.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def verify(target, directory, root=ROOT):
    manifest = json.loads((directory / "client.json").read_text(encoding="utf-8"))
    for key, value in identity(target, root).items():
        if manifest.get(key) != value:
            raise ValueError("Client artifact identity mismatch: " + key)
    filename = Path(TARGETS[target][1]).name
    if manifest.get("filename") != filename:
        raise ValueError("Unexpected client filename")
    binary = directory / filename
    if binary.is_symlink() or binary.stat().st_size != manifest.get("size") or digest(binary) != manifest.get("sha256"):
        raise ValueError("Client artifact checksum/size mismatch")
    print(f"Verified {target} client at {manifest['source_revision']}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["stage", "verify"])
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    (stage if args.command == "stage" else verify)(args.target, args.directory)
