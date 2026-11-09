"""Version checks and online release packaging. Python 3.11+, no Docker required."""
import argparse
import hashlib
import json
import re
import subprocess
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def version(tag=None, root=ROOT):
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    match = VERSION.fullmatch(cargo)
    if not match or (match[4] and any(x.isdigit() and len(x) > 1 and x[0] == "0" for x in match[4].split("."))):
        raise ValueError("Unsupported SemVer: " + cargo)
    versions = {
        "Cargo.toml": cargo,
        "tauri.conf.json": read_json(root / "tauri.conf.json")["version"],
        "ui/package.json": read_json(root / "ui/package.json")["version"],
        "package-lock.json": read_json(root / "package-lock.json")["packages"]["ui"]["version"],
        "Cargo.lock": next(p["version"] for p in tomllib.loads((root / "Cargo.lock").read_text())["package"] if p["name"] == "re-agent"),
    }
    if set(versions.values()) != {cargo}:
        raise ValueError("Versions differ: " + str(versions))
    if tag is not None and tag != "v" + cargo:
        raise ValueError(f"Tag {tag!r} does not match v{cargo}")
    return cargo


def validate_image(data, revision):
    if data.get("source_revision") != revision:
        raise ValueError("Image and client must use the same source commit")
    if data.get("platform") != "linux/amd64":
        raise ValueError("Only linux/amd64 tool images are supported")
    for field in ("digest", "image_id"):
        if not DIGEST.fullmatch(data.get(field, "")):
            raise ValueError("Invalid " + field)
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*/re-agent-workbench", data.get("repository", "")):
        raise ValueError("Invalid Docker Hub repository")
    if not isinstance(data.get("size_bytes"), int) or data["size_bytes"] <= 0:
        raise ValueError("Missing image size")


def package(binary, metadata, output, tag):
    number = version(tag)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    image = read_json(metadata)
    validate_image(image, revision)
    output.mkdir(parents=True, exist_ok=True)
    members = {
        "re-agent.exe": binary,
        "install-workbench.ps1": ROOT / "scripts/install-workbench.ps1",
        "rollback-workbench.ps1": ROOT / "scripts/rollback-workbench.ps1",
        "README.md": ROOT / "docs/online-install.md",
    }
    manifest = {
        "schema_version": 2, "version": number, "source_revision": revision,
        "platform": "windows-amd64", "tool_platform": "linux/amd64",
        "image": image["repository"] + "@" + image["digest"],
        "image_id": image["image_id"], "image_size_bytes": image["size_bytes"],
        "runtime_protocol": 1, "data_schema": 1,
        "files": {name: sha256(path) for name, path in members.items()},
    }
    write_json(output / "release.json", manifest)
    archive = output / f"re-agent-v{number}-windows-amd64.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        for name, path in members.items():
            bundle.write(path, name)
        bundle.write(output / "release.json", "release.json")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    check.add_argument("--tag")
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
    elif args.command == "package":
        print(json.dumps(package(args.binary, args.image_metadata, args.output, args.tag)))
    else:
        files = sorted(p for p in args.directory.iterdir() if p.is_file() and p.name != "SHA256SUMS")
        (args.directory / "SHA256SUMS").write_text("".join(f"{sha256(p)}  {p.name}\n" for p in files), encoding="utf-8")


if __name__ == "__main__":
    main()
