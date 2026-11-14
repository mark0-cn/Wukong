"""Acquire pinned official Windows assets on the controller, never in the VM."""
import argparse
import hashlib
import json
from pathlib import Path
import os
import re
import tempfile
import urllib.parse
import urllib.request

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts.worker.paths import member, no_links, regular, unique_paths

LOCK = ROOT / "workers/windows/toolchain.lock.json"
COMPONENTS = {"python", "debugging_tools", "openssh", "windbg_mcp"}
HOSTS = {"www.python.org", "download.microsoft.com", "github.com"}


def load_lock(path=LOCK):
    with regular(path).open("rb") as incoming:
        raw = incoming.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("Windows toolchain lock is too large")
    lock = json.loads(raw)
    if lock.get("schema") != 1 or lock.get("platform") != "windows-amd64":
        raise ValueError("Unsupported Windows toolchain lock")
    if set(lock.get("components", {})) != COMPONENTS:
        raise ValueError("Incomplete offline toolchain components")
    assets = lock.get("assets", [])
    if not 1 <= len(assets) <= 256:
        raise ValueError("Invalid offline asset count")
    unique_paths(item["path"] for item in assets)
    for item in assets:
        url = urllib.parse.urlsplit(item["url"])
        if (url.scheme != "https" or url.hostname not in HOSTS or url.username or url.password
                or url.fragment or url.port not in (None, 443)):
            raise ValueError("Offline asset must use an official HTTPS origin")
        if (item["component"] not in COMPONENTS or type(item["size"]) is not int
                or not 0 < item["size"] <= 2 * 1024**3
                or not re.fullmatch("[0-9a-f]{64}", item["sha256"])):
            raise ValueError("Invalid offline asset identity")
    if sum(item["size"] for item in assets) > 8 * 1024**3:
        raise ValueError("Offline toolchain exceeds 8 GiB")
    for name, component in lock["components"].items():
        if (not component.get("version") or component.get("channel") not in {"stable", "preview"}
                or not any(item["component"] == name for item in assets)):
            raise ValueError("Missing offline component payloads or version")
    return lock


def validate_asset(cache, item):
    path = regular(no_links(cache) / member(item["path"]))
    if path.stat().st_size != item["size"] or sha256(path) != item["sha256"]:
        raise ValueError("Pinned offline payload mismatch: " + item["path"])
    return path


class HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        # GitHub release objects redirect to a CDN; retain TLS on every hop.
        if urllib.parse.urlsplit(url).scheme != "https":
            raise ValueError("Insecure offline asset redirect")
        return super().redirect_request(request, response, code, message, headers, url)


def acquire(cache, lock, download=False):
    cache = no_links(cache)
    cache.mkdir(parents=True, exist_ok=True)
    for item in lock["assets"]:
        path = no_links(cache / member(item["path"]))
        if path.exists():
            validate_asset(cache, item)
            continue
        if not download:
            raise ValueError("Missing offline payload (controller download required): " + item["path"])
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".download-", delete=False) as output:
                temporary = Path(output.name)
                request = urllib.request.Request(item["url"], headers={"User-Agent": "Wukong-offline-preparation"})
                with urllib.request.build_opener(HttpsRedirect()).open(request, timeout=60) as response:
                    digest, size = hashlib.sha256(), 0
                    while chunk := response.read(1024 * 1024):
                        size += len(chunk)
                        if size > item["size"]:
                            raise ValueError("Downloaded offline payload exceeds pinned size")
                        digest.update(chunk)
                        output.write(chunk)
                if size != item["size"] or digest.hexdigest() != item["sha256"]:
                    raise ValueError("Downloaded offline payload does not match lock")
                output.flush()
                os.fsync(output.fileno())
            # Exclusive publication, including when a second preparer races us.
            try:
                os.link(temporary, path)
            except FileExistsError:
                validate_asset(cache, item)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
    return {"assets": len(lock["assets"]), "bytes": sum(item["size"] for item in lock["assets"])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--download", action="store_true", help="Allow downloads on the controller")
    parser.add_argument("--component", choices=sorted(COMPONENTS), help="Prepare only one component for a controller build")
    args = parser.parse_args()
    try:
        lock = load_lock()
        if args.component:
            lock["assets"] = [item for item in lock["assets"] if item["component"] == args.component]
        result = acquire(args.cache, lock, args.download)
        print(json.dumps(dict(result, toolchain_lock_sha256=sha256(LOCK))))
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Offline toolchain preparation failed: {error}\n")


if __name__ == "__main__":
    main()
