"""Build standalone Windows worker binaries with a verified static CRT."""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts._lib.versioning import write_json
from scripts.worker.pe import require_static_crt


def build(output, check_only=False):
    if platform.system() != "Windows" or platform.machine().lower() not in {"amd64", "x86_64"}:
        raise ValueError("Native Windows worker builds require Windows x64")
    cargo = shutil.which("cargo")
    if not cargo or not shutil.which("cl.exe") or not shutil.which("link.exe"):
        raise ValueError("Run in the x64 MSVC Native Tools environment with Cargo on PATH")
    source = ROOT / "workers/windows"
    if not (source / "Cargo.lock").is_file():
        raise ValueError("The standalone worker lockfile is missing")
    if check_only:
        return {"platform": "windows-amd64", "source": str(source), "output": str(output)}
    environment = os.environ.copy()
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", "target-feature=+crt-static"])
    subprocess.run([cargo, "build", "--locked", "--release", "--target", "x86_64-pc-windows-msvc",
                    "--target-dir", str(output)], cwd=source, env=environment, check=True)
    files = {}
    for name in ["wukong-dbgeng.exe"]:
        binary = output / "x86_64-pc-windows-msvc/release" / name
        files[name] = {"sha256": sha256(binary), "size": binary.stat().st_size,
                       "imported_libraries": require_static_crt(binary)}
    metadata = {"schema": 1, "platform": "windows-amd64", "static_crt_checked": True, "files": files}
    write_json(output / "native-build.json", metadata)
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/windows-worker-native")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        result = build(args.output.resolve(), args.check)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Windows worker build failed: {error}\n")
    import json
    print(json.dumps(result))


if __name__ == "__main__":
    main()
