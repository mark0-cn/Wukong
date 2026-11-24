"""Install Playwright system packages through the bounded APT mirror failover."""

import argparse
import re
import subprocess

from scripts.ci import install_apt


def missing_packages(browser):
    result = subprocess.run(
        ["npx", "--no-install", "playwright", "install-deps", "--dry-run", browser],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    if result.returncode not in (0, 1):
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"Playwright dependency probe failed: {detail}")
    if result.returncode == 0:
        if "All system dependencies are installed." not in result.stdout:
            raise RuntimeError("Playwright dependency probe returned an unrecognized result")
        return []

    packages = []
    reading_packages = False
    for line in result.stdout.splitlines():
        if re.fullmatch(r"Missing system dependencies \(\d+\):", line):
            reading_packages = True
            continue
        if not reading_packages:
            continue
        match = re.fullmatch(r"  ([^\s]+)", line)
        if match is None:
            raise RuntimeError(f"Unrecognized Playwright dependency output: {line!r}")
        packages.append(match.group(1))
    if not packages:
        raise RuntimeError("Playwright dependency probe failed without a package list")
    return packages


def install(browser):
    packages = missing_packages(browser)
    if packages:
        install_apt.install(packages)
    else:
        print("Playwright system dependencies are already installed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("browser", choices=["chromium"])
    args = parser.parse_args()
    try:
        install(args.browser)
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"{error}; Playwright dependency setup failed.\n")


if __name__ == "__main__":
    main()
