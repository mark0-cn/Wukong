"""Build the pinned WinDbg MCP source with the reviewed AppContainer stdin patch."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts.worker.layout import unpack_zip
from scripts.worker.notices import collect
from scripts.worker.paths import member, no_links, regular
from scripts.worker.toolchain import load_lock, validate_asset

REVISION = "91fba9283c5f9f9bb94cd7f9b04c0aa4f7885e8c"
PREFIX = f"windbg-mcp-{REVISION}/"
PATCH = ROOT / "workers/windows/patches/windbg-mcp-pipe-stdin.patch"
BINARY = "debugger/windbg-mcp.exe"
PATCH_MEMBER = "debugger/patches/" + PATCH.name
SUPPLEMENTAL_LICENSES = {
    ("iced-x86", "1.21.0"): ROOT / "workers/windows/licenses/iced-x86-1.21.0/LICENSE.txt",
}


def inputs():
    asset, = [row for row in load_lock()["assets"] if row["component"] == "windbg_mcp"]
    return asset, dict(upstream_revision=REVISION, source_sha256=asset["sha256"], patch_sha256=sha256(PATCH))


def build(cache, output):
    asset, provenance = inputs()
    archive = validate_asset(cache, asset)
    output = no_links(output)
    output.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment.pop("RUSTFLAGS", None)
    environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
    with tempfile.TemporaryDirectory(prefix=".windbg-build-", dir=output) as temporary:
        source = Path(temporary) / "source"
        unpack_zip(archive, source, prefix=PREFIX)
        # git apply also works outside a repository. Prevent discovery of the
        # controller checkout so patch paths remain relative to this source.
        environment["GIT_CEILING_DIRECTORIES"] = temporary
        for options in [["--check"], []]:
            subprocess.run(["git", "apply", *options, str(PATCH)], cwd=source, env=environment, check=True)
        target = output / "debugger-build"
        subprocess.run(["cargo", "build", "--locked", "--release", "--target", "x86_64-pc-windows-msvc",
                        "--target-dir", str(target)], cwd=source, env=environment, check=True)
        licenses = Path(temporary) / "licenses"
        notices = collect(licenses, source / "Cargo.toml", SUPPLEMENTAL_LICENSES)
        artifacts = [(target / "x86_64-pc-windows-msvc/release/windbg-mcp.exe", BINARY),
                     (PATCH, PATCH_MEMBER)]
        artifacts += [(path, "debugger/licenses/" + path.relative_to(licenses).as_posix()) for path in notices]
        files = {}
        for path, name in artifacts:
            destination = output / member(name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(regular(path), no_links(destination))
            files[name] = dict(size=destination.stat().st_size, sha256=sha256(destination))
    if provenance != inputs()[1]:
        raise ValueError("Debugger source inputs changed during the build")
    return dict(**provenance, files=files)


def validate(directory, record):
    if not isinstance(record, dict) or any(record.get(key) != value for key, value in inputs()[1].items()):
        raise ValueError("Debugger build does not match the pinned source and compatibility patch")
    files = record.get("files", {})
    if not {BINARY, PATCH_MEMBER}.issubset(files) or not any(name.startswith("debugger/licenses/") for name in files):
        raise ValueError("Debugger build is missing its binary, patch or license documents")
    for name, expected in files.items():
        if not member(name).startswith("debugger/"):
            raise ValueError("Debugger artifact is outside its build directory")
        path = regular(no_links(directory / name))
        if path.stat().st_size != expected["size"] or sha256(path) != expected["sha256"]:
            raise ValueError("Debugger build artifact changed: " + name)
