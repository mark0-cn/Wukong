"""Build a Windows online package locally, without publishing or changing versions."""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

from scripts._lib.repository import ROOT
from scripts._lib.versioning import read_json, version, write_json
from scripts.release.cli import checksums, package, validate_image
from scripts.release.release_metadata import DEFAULT_REPOSITORY, release_image


def checkout_identity():
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    changes = subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=normal"], cwd=ROOT, text=True
    ).strip()
    if changes:
        raise ValueError("Commit source changes first. Keep image metadata and build outputs under target/.")
    return revision


def run(command):
    print("Running: " + subprocess.list2cmdline([str(arg) for arg in command]), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def build(metadata, output, check_only=False, repository=DEFAULT_REPOSITORY):
    if platform.system() != "Windows" or platform.machine().lower() not in ("amd64", "x86_64"):
        raise ValueError("Local online packaging requires Windows x64 and the MSVC C++ build tools.")
    number = version()
    revision = checkout_identity()
    if output.exists():
        raise ValueError(f"Output already exists: {output}. Choose a new directory with --output.")
    npm = shutil.which("npm.cmd")
    cargo = shutil.which(os.environ.get("CARGO", "cargo"))
    if not npm or not cargo:
        raise ValueError("Node.js/npm and Cargo must be installed and available on PATH.")
    if metadata is not None:
        if not metadata.is_file():
            raise ValueError(f"Missing image metadata: {metadata}. Omit --image-metadata to download from Release.")
        image = read_json(metadata)
    else:
        image = release_image(number, revision, repository)
    validate_image(image, revision, released_version=number if metadata is None else None)
    print(f"Version: v{number}\nClient commit: {revision}\nImage commit: {image['source_revision']}"
          f"\nImage: {image['repository']}@{image['digest']}", flush=True)
    if check_only:
        print("Preflight passed. No build or package was created.")
        return

    run([npm, "ci"])
    run([npm, "run", "build"])
    target = "x86_64-pc-windows-msvc"
    run([cargo, "build", "--locked", "--release", "--features", "custom-protocol",
         "--target", target, "--target-dir", str(ROOT / "target")])
    binary = ROOT / "target" / target / "release" / "wukong.exe"
    if not binary.is_file():
        raise ValueError(f"Build did not produce the expected executable: {binary}")
    if checkout_identity() != revision or version() != number:
        raise ValueError("Source changed during the build. Retry on an unchanged checkout.")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="wukong-package-", dir=output.parent) as directory:
        staging = Path(directory)
        # Package exactly the metadata validated before the build, even if its file changes.
        snapshot = staging / "image.json"
        write_json(snapshot, image)
        bundle = staging / "package"
        package(binary, snapshot, bundle, "v" + number, released_image=metadata is None)
        checksums(bundle)
        # The destination must be new; never replace a previous package or unrelated files.
        bundle.rename(output)
    print(f"Package: {output / f'wukong-v{number}-windows-amd64.zip'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-metadata", type=Path,
                        help="Use a local same-commit image.json instead of downloading Release metadata")
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY,
                        help="Public GitHub repository for the exact version's Release (default: %(default)s)")
    parser.add_argument("--output", type=Path, default=Path("target/local-package"),
                        help="New output directory (default: %(default)s)")
    parser.add_argument("--check", action="store_true", help="Fetch and validate metadata without building")
    args = parser.parse_args()
    try:
        metadata = (ROOT / args.image_metadata).resolve() if args.image_metadata else None
        build(metadata, (ROOT / args.output).resolve(), args.check, args.repository)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Local packaging failed: {error}\n")


if __name__ == "__main__":
    main()
