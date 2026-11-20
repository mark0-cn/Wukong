"""Build and stage the Windows worker setup executable bundled in desktop installers."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from scripts._lib import msvc
from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts.worker import toolchain

SETUP_NAME = "WukongWorkerSetup.exe"
DEFAULT_OUTPUT = Path("target/worker-setup")
DEFAULT_CACHE = Path("target/windows-offline-cache")
# build_native -> build_debugger builds the pinned WinDbg MCP source; other
# offline components belong to worker media, not to the local native build.
TOOLCHAIN_COMPONENTS = ("windbg_mcp",)

EXECUTABLES = set(msvc.EXECUTABLES)


def resolve(path):
    path = Path(path)
    return path.resolve() if path.is_absolute() else (ROOT / path).resolve()


def ensure_writable_temporary():
    """Route Windows tool-chain temporary files into the repository workspace.

    Compilers and linkers create their temporary files through %%TMP%%/%%TEMP%%
    inherited from the packaging process. The system value can be an 8.3 short path
    whose target is not writable for child processes, which fails the link with
    LNK1104 even though the packaging process itself can create files. The workspace
    directory keeps the native build reproducible regardless of the host temp layout.
    """

    if os.name != "nt":
        return
    temporary = msvc.workspace_temporary(ROOT)
    os.environ["TMP"] = str(temporary)
    os.environ["TEMP"] = str(temporary)


def build_environment():
    """Return the x64 MSVC/temp environment shared by all native child processes."""

    try:
        return msvc.build_environment(ROOT)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise ValueError(
            "MSVC x64 build tools are unavailable; install Visual Studio C++ build tools "
            f"(vswhere/VsDevCmd discovery failed: {error})"
        ) from error


def native_source():
    source = ROOT / "workers/windows"
    if not (source / "Cargo.lock").is_file():
        raise ValueError(f"Native Windows worker source is missing: {source}")
    return source


def acquire_toolchain(cache):
    """Download and verify the pinned assets the native worker build requires."""

    lock = toolchain.load_lock()
    lock["assets"] = [item for item in lock["assets"] if item["component"] in TOOLCHAIN_COMPONENTS]
    return toolchain.acquire(cache, lock, download=True)


def toolchain_status(cache):
    lock = toolchain.load_lock()
    selected = [item for item in lock["assets"] if item["component"] in TOOLCHAIN_COMPONENTS]
    missing = [item["path"] for item in selected if not (cache / item["path"]).exists()]
    if missing:
        return {"available": False, "download_required": True, "missing": missing}
    toolchain.acquire(cache, {"assets": selected}, download=False)
    return {"available": True, "download_required": False, "missing": []}


def check(cache=DEFAULT_CACHE):
    """Validate real MSVC/SDK child processes, toolchain and native worker source."""

    environment = build_environment()
    missing = sorted(
        tool for tool in EXECUTABLES if shutil.which(tool, path=environment.get("PATH", "")) is None
    )
    if missing:
        raise ValueError("The x64 MSVC environment does not provide: " + ", ".join(missing))
    msvc.preflight(environment, ROOT)
    return {
        "platform": "windows-amd64",
        "msvc_installation": environment.get("VSINSTALLDIR"),
        "native_source": str(native_source()),
        "toolchain": toolchain_status(resolve(cache)),
    }


def run(command, environment=None):
    print("Running: " + subprocess.list2cmdline([str(arg) for arg in command]), flush=True)
    subprocess.run(command, cwd=ROOT, env=environment, check=True)


def build_and_stage(output, cache=DEFAULT_CACHE, environment=None):
    """Build the native worker, stage its bytes and return the staged identity."""

    if environment is None:
        ensure_writable_temporary()
        environment = build_environment()
    cache = resolve(cache)
    toolchain_result = acquire_toolchain(cache)
    run(
        [sys.executable, "-m", "scripts.worker.build_native", "--cache", str(cache)],
        environment=environment,
    )
    executable = ROOT / (
        "target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-worker.exe"
    )
    if not executable.is_file():
        raise ValueError(f"Native worker build did not produce {executable}")
    directory = resolve(output)
    if directory.exists() and not directory.is_dir():
        raise ValueError(f"Worker setup output is not a directory: {directory}")
    directory.mkdir(parents=True, exist_ok=True)
    destination = directory / SETUP_NAME
    shutil.copyfile(executable, destination)
    result = {
        "platform": "windows-amd64",
        "executable": str(destination),
        "sha256": sha256(destination),
        "size": destination.stat().st_size,
        "toolchain": toolchain_result,
    }
    print(
        f"{SETUP_NAME}: {result['size']} bytes, SHA256 {result['sha256']}",
        flush=True,
    )
    return result


def publish(result):
    """Record staging identity in the GitHub Actions step summary and outputs."""

    directory = Path(result["executable"]).parent
    try:
        directory = directory.relative_to(ROOT).as_posix()
    except ValueError:
        pass
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write(
                f"{SETUP_NAME} staged in {directory}: "
                f"{result['size']} bytes, SHA256 {result['sha256']}.\n"
            )
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as stream:
            stream.write(f"sha256={result['sha256']}\nsize={result['size']}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="Directory for the staged setup executable (default: %(default)s)",
    )
    parser.add_argument(
        "--cache",
        type=Path,
        default=DEFAULT_CACHE,
        help="Offline toolchain cache (default: %(default)s)",
    )
    parser.add_argument(
        "--check", action="store_true", help="Validate build entry points without building"
    )
    args = parser.parse_args()
    try:
        if args.check:
            result = check(args.cache)
        else:
            result = build_and_stage(args.output, args.cache)
            publish(result)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Windows worker setup preparation failed: {error}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
