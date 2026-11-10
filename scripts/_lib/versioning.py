"""Shared application version and checksum rules."""
import hashlib
import json
import re
import tomllib
from pathlib import Path
from scripts._lib.repository import ROOT

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


