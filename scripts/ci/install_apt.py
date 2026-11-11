"""Install Ubuntu CI dependencies from signed official sources with bounded waits.

Only disposable GitHub-hosted Ubuntu x64 runners may change their APT config.
The job-wide config also covers third-party installers such as Playwright.
"""
import argparse
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile


SOURCES = "/etc/apt/wukong-ci.sources"
MIRRORS = "/etc/apt/wukong-ci.mirrors"
EMPTY_SOURCES = "/etc/apt/wukong-ci-empty"
CONFIG = "/etc/apt/apt.conf.d/99wukong-ci"
LISTS = "/var/lib/apt/wukong-ci-lists"
KEYRING = "/usr/share/keyrings/ubuntu-archive-keyring.gpg"
UPDATE_SECONDS = 120
INSTALL_SECONDS = 480


def require_runner():
    if (platform.system() != "Linux" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
            or os.environ.get("RUNNER_OS") != "Linux"):
        raise ValueError("APT preparation only runs on disposable GitHub-hosted Linux runners.")
    release = platform.freedesktop_os_release()
    codename = release.get("VERSION_CODENAME")
    if (release.get("ID") != "ubuntu" or codename not in ("jammy", "noble")
            or platform.machine() != "x86_64"):
        raise ValueError("APT preparation supports Ubuntu 22.04/24.04 x64 runners only.")
    if not Path(KEYRING).is_file():
        raise ValueError("The Ubuntu archive signing keyring is unavailable.")
    return codename


def sources(codename):
    if codename not in ("jammy", "noble"):
        raise ValueError("Unsupported Ubuntu suite")
    return f"""Types: deb
URIs: mirror+file:{MIRRORS}
Suites: {codename} {codename}-updates {codename}-backports {codename}-security
Components: main restricted universe multiverse
Architectures: amd64
Signed-By: {KEYRING}
"""


def mirrors():
    # APT itself fails over per file, including package downloads by later callers.
    # Independent mirror domains avoid the observed *.ubuntu.com DNS outage.
    return ("https://mirrors.edge.kernel.org/ubuntu/\tpriority:1\n"
            "https://mirrors.mit.edu/ubuntu/\tpriority:2\n"
            "https://archive.ubuntu.com/ubuntu/\tpriority:3\n"
            "https://security.ubuntu.com/ubuntu/\tpriority:3 suite:jammy-security suite:noble-security\n")


def configuration():
    return f"""// Disposable GitHub runner only; retain all APT signature checks.
Dir::Etc::sourcelist "{SOURCES}";
Dir::Etc::sourceparts "{EMPTY_SOURCES}";
Dir::State::lists "{LISTS}";
APT::Update::Error-Mode "any";
Acquire::Retries "2";
Acquire::http::Timeout "15";
Acquire::https::Timeout "15";
DPkg::Lock::Timeout "60";
"""


def apt(arguments, seconds):
    command = ["sudo", "-n", "env", "DEBIAN_FRONTEND=noninteractive", "timeout",
               "--kill-after=15s", f"{seconds}s", "apt-get", *arguments]
    print(f"APT: {' '.join(arguments)} (limit {seconds}s)", flush=True)
    result = subprocess.run(command, check=False)
    if result.returncode:
        reason = "timed out" if result.returncode in (124, 137) else f"exited {result.returncode}"
        raise RuntimeError(f"APT {' '.join(arguments[:1])} {reason}; dependency setup failed, tests were not started.")


def install(packages):
    if not packages or any(not re.fullmatch(r"[a-z0-9][a-z0-9.+-]*[a-z0-9+]", p) for p in packages):
        raise ValueError("Expected explicit Ubuntu package names, without options or removal suffixes.")
    codename = require_runner()
    # Do not rewrite the runner's original source files, DNS or trust settings.
    # A separate list directory prevents stale image indexes masking update errors.
    with tempfile.TemporaryDirectory(prefix="wukong-ci-apt-") as directory:
        temporary = Path(directory)
        (temporary / "sources").write_text(sources(codename), encoding="utf-8")
        (temporary / "mirrors").write_text(mirrors(), encoding="utf-8")
        (temporary / "config").write_text(configuration(), encoding="utf-8")
        subprocess.run(["sudo", "-n", "install", "-d", "-m", "755", EMPTY_SOURCES, LISTS], check=True)
        for name, destination in (("sources", SOURCES), ("mirrors", MIRRORS), ("config", CONFIG)):
            subprocess.run(["sudo", "-n", "install", "-m", "644", str(temporary / name), destination], check=True)
    print(f"Ubuntu {codename}: HTTPS archive mirrors with APT failover; Ubuntu archive-keyring verification enabled.", flush=True)
    apt(["update"], UPDATE_SECONDS)
    apt(["install", "-y", *packages], INSTALL_SECONDS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packages", nargs="+")
    args = parser.parse_args()
    try:
        install(args.packages)
    except (ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
