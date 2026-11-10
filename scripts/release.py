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
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
RELEASE_TAG = re.compile("v" + VERSION.pattern)
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
VERSION_FILES = ("Cargo.toml", "Cargo.lock", "tauri.conf.json", "ui/package.json", "package-lock.json")


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate_tag(tag):
    if not RELEASE_TAG.fullmatch(tag):
        raise ValueError(f"Invalid release tag {tag!r}: expected vMAJOR.MINOR.PATCH (e.g. v0.1.0), ASCII digits only, no leading zeros or suffixes")
    return tag[1:]


def version(tag=None, root=ROOT):
    if tag is not None:
        validate_tag(tag)
    package = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]
    cargo = package["version"]
    toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]
    if toolchain != package["rust-version"]:
        raise ValueError("Rust toolchain and Cargo rust-version differ")
    if not VERSION.fullmatch(cargo):
        raise ValueError("Release version must be MAJOR.MINOR.PATCH without leading zeros or suffixes: " + cargo)
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


def bump_version(tag, root=ROOT):
    """Update only the application versions; never resolve or upgrade dependencies."""
    target = validate_tag(tag)
    current = version(root=root)
    if tuple(map(int, target.split("."))) <= tuple(map(int, current.split("."))):
        raise ValueError(f"New version {target} must be greater than {current}")
    original = {name: (root / name).read_bytes() for name in VERSION_FILES}
    updated = {}
    for name, raw in original.items():
        source = raw.decode("utf-8")
        if name in ("Cargo.toml", "Cargo.lock"):
            header = "[package]" if name == "Cargo.toml" else "[[package]]"
            blocks = re.split(r"(?=^\[)", source, flags=re.MULTILINE)
            matched = 0
            for index, block in enumerate(blocks):
                if block.splitlines()[:1] != [header]:
                    continue
                data = tomllib.loads(block)["package"]
                package_data = data if name == "Cargo.toml" else data[0]
                if package_data.get("name") != "re-agent":
                    continue
                blocks[index], count = re.subn(r'^version = "' + re.escape(current) + r'"(\r?)$',
                                               lambda match: f'version = "{target}"' + match[1],
                                               block, flags=re.MULTILINE)
                matched += count
            if matched != 1:
                raise ValueError(f"Expected one application version in {name}, got {matched}")
            updated[name] = "".join(blocks).encode("utf-8")
        else:
            data = json.loads(source)
            if name == "package-lock.json":
                data["packages"]["ui"]["version"] = target
            else:
                data["version"] = target
            newline = "\r\n" if "\r\n" in source else "\n"
            updated[name] = (json.dumps(data, indent=2, ensure_ascii=False) + "\n").replace("\n", newline).encode("utf-8")
    try:
        for name, raw in updated.items():
            (root / name).write_bytes(raw)
        version(tag, root=root)
    except Exception:
        for name, raw in original.items():
            (root / name).write_bytes(raw)
        raise
    return target


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
        files = sorted(p for p in args.directory.iterdir() if p.is_file() and p.name != "SHA256SUMS")
        (args.directory / "SHA256SUMS").write_text("".join(f"{sha256(p)}  {p.name}\n" for p in files), encoding="utf-8")


if __name__ == "__main__":
    main()
