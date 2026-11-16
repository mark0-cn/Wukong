"""Prepare Ubuntu CI packages with bounded whole-mirror failover.

Only disposable hosted runners may change APT configuration. Download retries
never interrupt dpkg: installation starts locally after all downloads succeed.
"""

import argparse
import math
import os
from pathlib import Path
import platform
import re
import secrets
import subprocess
import tempfile
import time


SOURCES = "/etc/apt/wukong-ci.sources"
EMPTY_SOURCES = "/etc/apt/wukong-ci-empty"
CONFIG = "/etc/apt/apt.conf.d/99wukong-ci"
LISTS = "/var/lib/apt/wukong-ci-lists"
KEYRING = "/usr/share/keyrings/ubuntu-archive-keyring.gpg"
MIRRORS = (
    "https://mirrors.edge.kernel.org/ubuntu/",
    "https://mirrors.mit.edu/ubuntu/",
    "https://archive.ubuntu.com/ubuntu/",
)
UPDATE_SECONDS = 40
DOWNLOAD_SECONDS = 120
NETWORK_SECONDS = 480
INSTALL_SECONDS = 180
KILL_SECONDS = 5


def require_runner():
    if (
        platform.system() != "Linux"
        or os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
        or os.environ.get("RUNNER_OS") != "Linux"
    ):
        raise ValueError("APT preparation only runs on disposable GitHub-hosted Linux runners.")
    release = platform.freedesktop_os_release()
    codename = release.get("VERSION_CODENAME")
    if (
        release.get("ID") != "ubuntu"
        or codename not in ("jammy", "noble")
        or platform.machine() != "x86_64"
    ):
        raise ValueError("APT preparation supports Ubuntu 22.04/24.04 x64 runners only.")
    if not Path(KEYRING).is_file():
        raise ValueError("The Ubuntu archive signing keyring is unavailable.")
    return codename


def sources(codename, mirror):
    if codename not in ("jammy", "noble"):
        raise ValueError("Unsupported Ubuntu suite")
    if not re.fullmatch(r"https://[a-z0-9.-]+/ubuntu/", mirror):
        raise ValueError("Expected an HTTPS Ubuntu mirror")
    security = (
        "https://security.ubuntu.com/ubuntu/"
        if mirror == "https://archive.ubuntu.com/ubuntu/"
        else mirror
    )
    return "\n".join(
        f"""Types: deb
URIs: {uri}
Suites: {suites}
Components: main restricted universe multiverse
Architectures: amd64
Signed-By: {KEYRING}
"""
        for uri, suites in (
            (mirror, f"{codename} {codename}-updates {codename}-backports"),
            (security, f"{codename}-security"),
        )
    )


def configuration(lists):
    return f"""// Disposable GitHub runner only; retain all APT signature checks.
Dir::Etc::sourcelist "{SOURCES}";
Dir::Etc::sourceparts "{EMPTY_SOURCES}";
Dir::State::lists "{lists}";
APT::Update::Error-Mode "any";
Acquire::Retries "0";
Acquire::http::Timeout "10";
Acquire::https::Timeout "10";
Acquire::Languages "none";
Acquire::IndexTargets::deb::CNF::DefaultEnabled "false";
DPkg::Lock::Timeout "30";
"""


def installed(package):
    result = subprocess.run(
        ["dpkg-query", "-W", "-f=${Status}", package],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    return result.returncode == 0 and result.stdout == "install ok installed"


def verify(packages):
    missing = [package for package in packages if not installed(package)]
    if missing:
        raise RuntimeError("Required packages are not installed: " + ", ".join(missing))
    commands = {
        "gdb": ["gdb", "--version"],
        "patchelf": ["patchelf", "--version"],
        "musl-tools": ["musl-gcc", "--version"],
        "gcc-multilib": ["gcc", "--version"],
    }
    modules = {
        "libwayland-dev": "wayland-client",
        "libxkbcommon-dev": "xkbcommon",
        "libwebkit2gtk-4.1-dev": "webkit2gtk-4.1",
        "libayatana-appindicator3-dev": "ayatana-appindicator3-0.1",
        "librsvg2-dev": "librsvg-2.0",
    }
    for package in packages:
        command = commands.get(package)
        if package in modules:
            command = ["pkg-config", "--modversion", modules[package]]
        if command:
            subprocess.run(command, check=True, timeout=15)
    print("APT: required packages and tool probes passed", flush=True)


def configure(source, lists):
    # Each attempt gets fresh indexes; partial/stale indexes never authorize install.
    with tempfile.TemporaryDirectory(prefix="wukong-ci-apt-") as directory:
        temporary = Path(directory)
        (temporary / "sources").write_text(source, encoding="utf-8")
        (temporary / "config").write_text(configuration(lists), encoding="utf-8")
        subprocess.run(
            ["sudo", "-n", "install", "-d", "-m", "755", EMPTY_SOURCES, lists],
            check=True,
            timeout=15,
        )
        for name, destination in (("sources", SOURCES), ("config", CONFIG)):
            subprocess.run(
                ["sudo", "-n", "install", "-m", "644", str(temporary / name), destination],
                check=True,
                timeout=15,
            )


def apt(arguments, seconds):
    command = [
        "sudo",
        "-n",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "timeout",
        f"--kill-after={KILL_SECONDS}s",
        f"{seconds}s",
        "apt-get",
        *arguments,
    ]
    print(f"APT: {' '.join(arguments)} (limit {seconds}s)", flush=True)
    result = subprocess.run(command, check=False)
    if result.returncode:
        reason = "timed out" if result.returncode in (124, 137) else f"exited {result.returncode}"
        raise RuntimeError(f"APT {arguments[0]} {reason}")


def download(packages, candidates):
    deadline = time.monotonic() + NETWORK_SECONDS
    attempt = secrets.token_hex(8)
    failures = []
    for index, (label, source) in enumerate(candidates):
        if deadline - time.monotonic() <= KILL_SECONDS:
            break
        print(f"APT: trying mirror {label}", flush=True)
        try:
            configure(source, f"{LISTS}/{attempt}-{index}")
            for arguments, limit in (
                (["update"], UPDATE_SECONDS),
                (["install", "--download-only", "-y", *packages], DOWNLOAD_SECONDS),
            ):
                remaining = math.floor(deadline - time.monotonic()) - KILL_SECONDS
                if remaining <= 0:
                    raise RuntimeError("APT network budget exhausted")
                apt(arguments, min(limit, remaining))
            print(f"APT: selected mirror {label}; downloads complete", flush=True)
            return
        except RuntimeError as error:
            failures.append(f"{label}: {error}")
            print(f"APT: abandoning mirror {label}: {error}", flush=True)
    raise RuntimeError("All APT mirror attempts failed or timed out: " + "; ".join(failures))


def install(packages):
    if not packages or any(not re.fullmatch(r"[a-z0-9][a-z0-9.+-]*[a-z0-9+]", p) for p in packages):
        raise ValueError(
            "Expected explicit Ubuntu package names, without options or removal suffixes."
        )
    codename = require_runner()
    packages = list(dict.fromkeys(packages))
    missing = [package for package in packages if not installed(package)]
    if missing:
        download(missing, [(mirror, sources(codename, mirror)) for mirror in MIRRORS])
        apt(["install", "--no-download", "-y", *missing], INSTALL_SECONDS)
    else:
        print("APT: all requested packages are preinstalled; skipping network", flush=True)
    verify(packages)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packages", nargs="+")
    args = parser.parse_args()
    try:
        install(args.packages)
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"{error}; dependency setup failed, tests were not started.\n")


if __name__ == "__main__":
    main()
