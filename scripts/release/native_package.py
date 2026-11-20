"""Build the host platform's standalone desktop installation file.

By default this is a local-release package: it bundles `local-release.json` and
enables the `local-release` Cargo feature so first-run onboarding can use a
locally built Docker image. `--online` restores the previous online-catalog
installer (the image is pulled from the release catalog at first onboarding).
"""

import argparse
import platform
from pathlib import Path
import shutil
from scripts._lib.versioning import ROOT
from scripts.release import worker_setup
from scripts.release.local_package import checkout_identity
from scripts.release.build_installer import build


ARTIFACT_SUFFIXES = ("-setup.exe", ".AppImage", ".sig")


def prepare_output(output):
    """Reject unsafe output paths and allow replacing a previous package output.

    Re-running the packaging command is common during local development, so an
    existing directory that only holds installers written by this command is
    replaced after a successful build. Directories with unrelated content are
    never deleted; they must be cleared by the developer explicitly.
    """

    if output.is_symlink():
        raise ValueError("Output must not be a symbolic link.")
    if not output.exists():
        return
    if not output.is_dir():
        raise ValueError("Output exists and is not a directory.")
    if any(not entry.is_file() for entry in output.iterdir()):
        raise ValueError("Output contains subdirectories; choose another directory.")
    if any(not entry.name.endswith(ARTIFACT_SUFFIXES) for entry in output.iterdir()):
        raise ValueError(
            "Output contains files not created by package-local; choose another directory."
        )


def replace_output(output):
    """Replace a validated previous output directory with the new package."""

    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)


def package(output, *, signed=False, check=False, local_release=True):
    system = platform.system()
    if system not in ("Windows", "Linux") or platform.machine().lower() not in ("amd64", "x86_64"):
        raise ValueError("Native packages support Windows x64 and Linux x64.")
    revision = checkout_identity()
    worker_setup.ensure_writable_temporary()
    prepare_output(output)
    if not shutil.which("cargo") or not shutil.which("npm.cmd" if system == "Windows" else "npm"):
        raise ValueError("Cargo and Node.js/npm must be on PATH.")
    if check:
        if system == "Windows":
            worker = worker_setup.check()
            toolchain = worker["toolchain"]
            print(
                "Windows worker setup: MSVC x64 found; offline toolchain "
                + ("ready" if toolchain["available"] else "downloaded on first build")
                + "; native worker source present.",
                flush=True,
            )
        print(
            "Source and tool entry points checked. Windows preflight also compiles, archives and links an SDK probe; other system libraries, Docker access and installer tools are checked during build."
        )
        return
    build("windows" if system == "Windows" else "linux", signed, local_release=local_release)
    if checkout_identity() != revision:
        raise ValueError("Source changed while building.")
    pattern = "nsis/*-setup.exe" if system == "Windows" else "appimage/*.AppImage"
    installers = list((ROOT / "target/release/bundle").glob(pattern))
    if len(installers) != 1:
        raise ValueError("Expected one installer; remove stale build outputs before retrying.")
    installer = installers[0]
    replace_output(output)
    shutil.copy2(installer, output / installer.name)
    if signed:
        signature = installer.with_name(installer.name + ".sig")
        shutil.copy2(signature, output / signature.name)
    print("Native installer: " + str(output / installer.name))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("target/local-installers"))
    parser.add_argument("--signed", action="store_true")
    parser.add_argument("--check", action="store_true")
    parser.add_argument(
        "--online",
        action="store_true",
        help="Build the online-catalog installer instead of the local-release package",
    )
    args = parser.parse_args()
    package(ROOT / args.output, signed=args.signed, check=args.check, local_release=not args.online)
