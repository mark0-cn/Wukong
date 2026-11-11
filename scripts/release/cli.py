"""Version CLI and online release packaging."""
import argparse
import json
import re
import subprocess
import zipfile
from pathlib import Path
from scripts._lib.repository import ROOT
from scripts._lib.versioning import DIGEST, bump_version, read_json, sha256, version, write_json


def validate_image(data, revision, *, released_version=None):
    if released_version is None and data.get("source_revision") != revision:
        raise ValueError("Image and client must use the same source commit")
    if released_version is not None:
        if data.get("release_version") != released_version:
            raise ValueError("Image Release version must match the client version")
        if not re.fullmatch(r"[0-9a-f]{40}", data.get("source_revision", "")):
            raise ValueError("Image Release must record its original source commit")
    if data.get("platform") != "linux/amd64":
        raise ValueError("Only linux/amd64 tool images are supported")
    for field in ("digest", "image_id"):
        if not DIGEST.fullmatch(data.get(field, "")):
            raise ValueError("Invalid " + field)
    name = r"(?:wukong|re-agent-workbench)" if released_version is not None else "wukong"
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*/" + name, data.get("repository", "")):
        raise ValueError("Invalid Docker Hub repository")
    if not isinstance(data.get("size_bytes"), int) or data["size_bytes"] <= 0:
        raise ValueError("Missing image size")


def package(binary, metadata, output, tag, *, released_image=False):
    number = version(tag)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    image = read_json(metadata)
    validate_image(image, revision, released_version=number if released_image else None)
    output.mkdir(parents=True, exist_ok=True)
    members = {
        "wukong.exe": binary,
        "install-workbench.ps1": ROOT / "packaging/windows/install-workbench.ps1",
        "rollback-workbench.ps1": ROOT / "packaging/windows/rollback-workbench.ps1",
        "README.md": ROOT / "docs/user/online-install.md",
    }
    manifest = {
        "schema_version": 2, "version": number, "source_revision": revision,
        "image_source_revision": image["source_revision"],
        "platform": "windows-amd64", "tool_platform": "linux/amd64",
        "image": image["repository"] + "@" + image["digest"],
        "image_id": image["image_id"], "image_size_bytes": image["size_bytes"],
        "runtime_protocol": 2, "data_schema": 4,
        "files": {name: sha256(path) for name, path in members.items()},
    }
    if released_image:
        manifest["image_release_version"] = number
    write_json(output / "release.json", manifest)
    archive = output / f"wukong-v{number}-windows-amd64.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        for name, path in members.items():
            bundle.write(path, name)
        bundle.write(output / "release.json", "release.json")
    return manifest


def checksums(directory):
    files = sorted(p for p in directory.iterdir() if p.is_file() and p.name != "SHA256SUMS")
    (directory / "SHA256SUMS").write_text("".join(f"{sha256(p)}  {p.name}\n" for p in files), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    check.add_argument("--tag")
    bump = commands.add_parser("bump", help="Synchronize all application versions without changing dependencies")
    bump.add_argument("--tag", required=True)
    pack = commands.add_parser("package")
    pack.add_argument("--tag", required=True)
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--image-metadata", type=Path, required=True)
    pack.add_argument("--output", type=Path, required=True)
    sums = commands.add_parser("checksums")
    sums.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "check":
        print(version(args.tag))
    elif args.command == "bump":
        print(bump_version(args.tag))
    elif args.command == "package":
        print(json.dumps(package(args.binary, args.image_metadata, args.output, args.tag)))
    else:
        checksums(args.directory)


if __name__ == "__main__":
    main()
