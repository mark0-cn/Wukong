"""Exercise actual signed APT and slow-server failover on disposable hosted runners."""

from contextlib import contextmanager
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import sys
import threading
import time
from unittest.mock import patch
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.ci import install_apt as apt


def original_sources():
    paths = [Path("/etc/apt/sources.list"), *Path("/etc/apt/sources.list.d").glob("*")]
    return {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths if path.is_file()
    }


def healthy_index(suite):
    for mirror in apt.MIRRORS:
        try:
            with urllib.request.urlopen(f"{mirror}dists/{suite}/InRelease", timeout=10) as response:
                data = response.read(2 * 1024 * 1024)
            if not data.startswith(b"-----BEGIN PGP SIGNED MESSAGE-----"):
                raise ValueError("Fixture did not receive a signed InRelease")
            return mirror, data
        except (OSError, ValueError) as error:
            print(f"Fixture mirror unavailable: {mirror}: {error}", flush=True)
    raise RuntimeError("No official signed index is reachable for the acceptance fixture")


def source(suite, uri):
    return f"Types: deb\nURIs: {uri}\nSuites: {suite}\nComponents: main\nArchitectures: amd64\nSigned-By: {apt.KEYRING}\n"


@contextmanager
def slow_server(index=None):
    release, reached, index_requested = threading.Event(), threading.Event(), threading.Event()
    sizes = {}
    if index is not None:
        for line in index.decode("utf-8").splitlines():
            fields = line.split()
            if len(fields) == 3 and len(fields[0]) == 64 and fields[1].isdigit():
                sizes[fields[0]] = int(fields[1])

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            reached.set()
            try:
                if index is not None and self.path.endswith("/InRelease"):
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(index)))
                    self.end_headers()
                    self.wfile.write(index)
                    self.wfile.flush()
                    return
                if index is not None:
                    size = sizes.get(self.path.rsplit("/", 1)[-1])
                    if size is None:
                        self.send_error(404)
                        return
                    index_requested.set()
                    # Signed metadata was already served. Stall after a partial
                    # index body to exercise read timeout, not connection refusal.
                    self.send_response(200)
                    # Match the original signed size so APT really waits for
                    # bytes instead of rejecting the headers immediately.
                    self.send_header("Content-Length", str(size))
                    self.end_headers()
                    self.wfile.write(b"partial-index")
                    self.wfile.flush()
                release.wait(30)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/ubuntu/", reached, index_requested
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(5)


def main():
    suite = apt.require_runner()
    before = original_sources()
    mirror, signed = healthy_index(suite)
    # Force real downloads for an absent small package, without relying on the
    # hosted image's incidental package inventory.
    package = next((p for p in ("hello", "sl", "toilet") if not apt.installed(p)), None)
    if package is None:
        raise AssertionError("All acceptance fixture packages are unexpectedly preinstalled")
    good = [(mirror, source(suite, mirror)) for mirror in apt.MIRRORS]
    for mode, index in [("silent-server", None), ("partial-index", signed)]:
        with slow_server(index) as (uri, reached, index_requested):
            started = time.monotonic()
            real_apt = apt.apt
            failures = []

            def bounded_fixture(arguments, seconds):
                current = Path(apt.SOURCES).read_text(encoding="utf-8")
                try:
                    real_apt(arguments, min(seconds, 4) if uri in current else seconds)
                except RuntimeError as error:
                    failures.append(str(error))
                    raise

            with patch.object(apt, "apt", side_effect=bounded_fixture):
                apt.download([package], [(mode, source(suite, uri)), *good])
            if not reached.is_set() or (index is not None and not index_requested.is_set()):
                raise AssertionError(f"Slow-server fixture was not exercised: {mode}")
            if not failures or "timed out" not in failures[0]:
                raise AssertionError(f"Expected a real read timeout, got {failures}")
            # The failed mirror must have been abandoned for all subsequent reads.
            active = Path(apt.SOURCES).read_text(encoding="utf-8")
            if uri in active or not any(mirror in active for mirror in apt.MIRRORS):
                raise AssertionError("Failed mirror remained in the selected APT source")
            print(
                f"{mode}: entire mirror abandoned; signed fallback passed in {time.monotonic() - started:.1f}s",
                flush=True,
            )
    apt.apt(["install", "--no-download", "-y", package], apt.INSTALL_SECONDS)
    apt.verify([package])
    with patch.object(
        apt, "download", side_effect=AssertionError("Preinstalled package used network")
    ):
        apt.install([package])
    config = subprocess.check_output(["apt-config", "dump"], text=True, timeout=10)
    for expected in (f'Dir::Etc::sourcelist "{apt.SOURCES}";', 'APT::Update::Error-Mode "any";'):
        if expected not in config:
            raise AssertionError(f"Indirect APT caller configuration missing: {expected}")
    # No reachable mirror must remain a hard failure, even with earlier valid indexes.
    selected_source = Path(apt.SOURCES).read_text(encoding="utf-8")
    selected_config = Path(apt.CONFIG).read_text(encoding="utf-8")
    with slow_server() as (uri, reached, _):
        started = time.monotonic()
        try:
            with patch.object(apt, "UPDATE_SECONDS", 3):
                apt.download([package], [("only-slow-mirror", source(suite, uri))])
        except RuntimeError:
            pass
        else:
            raise AssertionError("All-mirror failure was accepted using stale indexes")
        if not reached.is_set() or time.monotonic() - started > 15:
            raise AssertionError("Silent mirror did not respect the bounded attempt")
    # Restore only this fixture's own job configuration for subsequent callers.
    lists = selected_config.split('Dir::State::lists "', 1)[1].split('"', 1)[0]
    apt.configure(selected_source, lists)
    if original_sources() != before:
        raise AssertionError("APT acceptance changed the runner's original sources")
    print(
        f"Ubuntu {suite}: signed fallback, slow headers/body, offline install, preinstalled reuse and strict failure passed."
    )


if __name__ == "__main__":
    main()
