"""Exercise actual APT on disposable hosted Ubuntu runners, including failure.

This does not install or uninstall Wukong or touch a developer's package manager.
"""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.ci import install_apt as apt


def original_sources():
    paths = [Path("/etc/apt/sources.list"), *Path("/etc/apt/sources.list.d").glob("*")]
    return {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths if path.is_file()}


def main():
    suite = apt.require_runner()  # Refuse local and self-hosted machines before any mutation.
    before = original_sources()
    apt.install(["patchelf", "libwayland-dev"])
    if original_sources() != before:
        raise AssertionError("CI setup changed the runner's original sources")
    for package in ("patchelf", "libwayland-dev"):
        status = subprocess.check_output(["dpkg-query", "-W", "-f=${Status}", package], text=True, timeout=10)
        if status != "install ok installed":
            raise AssertionError(f"Package was not installed: {package}")
    # An indirect apt caller (such as Playwright) must use the same configuration.
    config = subprocess.check_output(["apt-config", "dump"], text=True, timeout=10)
    for expected in (f'Dir::Etc::sourcelist "{apt.SOURCES}";', 'APT::Update::Error-Mode "any";'):
        if expected not in config:
            raise AssertionError(f"Job-wide APT configuration missing: {expected}")
    with tempfile.TemporaryDirectory(prefix="wukong-apt-failure-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        mirrorlist = root / "fallback.mirrors"
        mirrorlist.write_text("http://127.0.0.1:1/ubuntu/\tpriority:0\n" + apt.mirrors(), encoding="utf-8")
        fallback_source = root / "fallback.sources"
        fallback_source.write_text(f"Types: deb\nURIs: mirror+file:{mirrorlist}\nSuites: {suite}\n"
                                   f"Components: main\nArchitectures: amd64\nSigned-By: {apt.KEYRING}\n", encoding="utf-8")
        for file in (mirrorlist, fallback_source):
            file.chmod(0o644)
        source = root / "failure.sources"
        source.write_text(f"Types: deb\nURIs: http://127.0.0.1:1/ubuntu\nSuites: {suite}\n"
                          f"Components: main\nSigned-By: {apt.KEYRING}\n", encoding="utf-8")
        source.chmod(0o644)
        lists = root / "lists"
        lists.mkdir(mode=0o755)
        fallback_lists = root / "fallback-lists"
        fallback_lists.mkdir(mode=0o755)
        command = ["sudo", "-n", "timeout", "--kill-after=5s", "15s", "apt-get",
                   "-o", f"Dir::Etc::sourcelist={source}", "-o", f"Dir::State::lists={lists}",
                   "-o", "Acquire::Retries=0", "-o", "Acquire::http::Timeout=2", "update"]
        try:
            fallback_command = ["sudo", "-n", "timeout", "--kill-after=15s", "120s", "apt-get",
                                "-o", f"Dir::Etc::sourcelist={fallback_source}",
                                "-o", f"Dir::State::lists={fallback_lists}", "-o", "Acquire::Retries=0",
                                "-o", "Acquire::http::Timeout=2", "update"]
            fallback = subprocess.run(fallback_command, capture_output=True, text=True, timeout=140)
            print(fallback.stdout, flush=True)
            print(fallback.stderr, file=sys.stderr, flush=True)
            if fallback.returncode != 0 or "127.0.0.1" not in fallback.stdout + fallback.stderr:
                raise AssertionError(f"APT did not recover from the unreachable first mirror: exit {fallback.returncode}")
            result = subprocess.run(command, capture_output=True, text=True, timeout=25)
            print(result.stdout, flush=True)
            print(result.stderr, file=sys.stderr, flush=True)
            if result.returncode != 100 or "Failed to fetch" not in result.stderr:
                raise AssertionError(f"Index download failure was not rejected by APT: exit {result.returncode}")
        finally:
            # Root-owned files can exist only under this exact temporary fixture.
            for owned in (lists, fallback_lists):
                assert owned.resolve().parent == root.resolve() and owned.name in ("lists", "fallback-lists")
                subprocess.run(["sudo", "-n", "rm", "-rf", "--", str(owned)], check=True, timeout=10)
    if original_sources() != before:
        raise AssertionError("APT failure fixture changed the runner's original sources")
    print(f"Ubuntu {suite}: signed package install, indirect caller configuration, mirror failover and strict index failure passed.")


if __name__ == "__main__":
    main()
