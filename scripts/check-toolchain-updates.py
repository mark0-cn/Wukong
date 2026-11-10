"""Read upstream releases and propose reviewed pins; never modify the lock or run downloads."""
import argparse
import base64
from datetime import datetime, timezone
import hashlib
import html
import json
import os
from pathlib import Path
import re
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {
    "pwndbg": ("pwndbg/pwndbg", r"v?(\d+\.\d+\.\d+)", r"pwndbg_[\d.]+_x86_64-portable\.tar\.xz"),
    "pwninit": ("io12/pwninit", r"v?(\d+\.\d+\.\d+)", r"pwninit"),
    "ghidra": ("NationalSecurityAgency/ghidra", r"Ghidra_(\d+(?:\.\d+)+)_build", r"ghidra_[\d.]+_PUBLIC_\d+\.zip"),
    "temurin": ("adoptium/temurin25-binaries", r"jdk-(25\.\d+\.\d+\+\d+)", r"OpenJDK25U-jdk_x64_linux_hotspot_[\d._]+\.tar\.gz"),
    "gef": ("hugsy/gef", r"(\d{4}\.\d{2})", None),
}


def api(path):
    # No redirects: credentials must only reach api.github.com.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "re-agent-toolchain-check"}
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request("https://api.github.com" + path, headers=headers)
    for attempt in range(3):
        try:
            with urllib.request.build_opener(NoRedirect()).open(request, timeout=30) as response:
                data = response.read(4 * 1024 * 1024 + 1)
            if len(data) > 4 * 1024 * 1024:
                raise ValueError("GitHub response exceeded 4 MiB")
            return json.loads(data)
        except urllib.error.HTTPError as error:
            if error.code < 500 or attempt == 2:
                raise RuntimeError(f"GitHub API returned HTTP {error.code} for {path}") from error
        except urllib.error.URLError:
            if attempt == 2:
                raise
        time.sleep(attempt + 1)


def version_key(value):
    if not re.fullmatch(r"\d+(?:[.+]\d+)+", value):
        raise ValueError("Unrecognized version: " + value)
    parts = tuple(map(int, re.split(r"[.+]", value)))
    return parts + (0,) * max(0, 5 - len(parts))


def inspect(name, current, get=api):
    repository, tag_pattern, asset_pattern = SOURCES[name]
    release = get(f"/repos/{repository}/releases/latest")
    tag = release["tag_name"]
    match = re.fullmatch(tag_pattern, tag)
    if not match or release.get("draft") or release.get("prerelease"):
        raise ValueError("Unexpected stable release tag: " + tag)
    latest = match[1]
    row = {"tool": name, "current": current["version"], "latest": latest,
           "release_url": f"https://github.com/{repository}/releases/tag/{urllib.parse.quote(tag, safe='')}",
           "status": "current", "checksum_source": "", "candidate": None}
    if version_key(latest) < version_key(current["version"]):
        row["status"] = "upstream_older"
        return row
    if name == "gef":
        # GEF publishes source tags, not a release asset with an independent SHA256.
        commit = get(f"/repos/{repository}/commits/{urllib.parse.quote(tag, safe='')}")["sha"]
        if not re.fullmatch(r"[a-f0-9]{40}", commit):
            raise ValueError("Invalid upstream commit")
        source = get(f"/repos/{repository}/contents/gef.py?ref={commit}")
        if source.get("encoding") != "base64" or source.get("type") != "file":
            raise ValueError("GEF source is not a base64-encoded regular file")
        content = base64.b64decode("".join(source["content"].split()), validate=True)
        blob = hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest()
        if blob != source["sha"] or len(content) != source["size"]:
            raise ValueError("GEF source blob identity mismatch")
        digest = hashlib.sha256(content).hexdigest()
        url = f"https://raw.githubusercontent.com/{repository}/{commit}/gef.py"
        row["checksum_source"] = "computed from GitHub source at " + commit + "; review source diff"
    else:
        assets = [a for a in release["assets"] if re.fullmatch(asset_pattern, a["name"])]
        if len(assets) != 1:
            raise ValueError("Expected exactly one linux/amd64 asset, found " + str(len(assets)))
        asset = assets[0]
        url = asset["browser_download_url"]
        parsed = urllib.parse.urlparse(url)
        expected = f"/{repository}/releases/download/{tag}/{asset['name']}"
        if parsed.scheme != "https" or parsed.netloc != "github.com" or urllib.parse.unquote(parsed.path) != expected or parsed.query or parsed.fragment:
            raise ValueError("Unexpected release asset URL")
        published_digest = asset.get("digest") or ""
        if not re.fullmatch(r"sha256:[a-f0-9]{64}", published_digest):
            row.update(status="checksum_review_required", checksum_source="No GitHub asset SHA256; inspect upstream checksums manually")
            return row
        digest = published_digest.split(":", 1)[1]
        row["checksum_source"] = "GitHub release asset SHA256; download verification pending"
    if version_key(latest) == version_key(current["version"]):
        if digest != current["sha256"]:
            row["status"] = "checksum_changed"
            row["upstream_sha256"] = digest
        return row
    row["status"] = "source_update_review" if name == "gef" else "update_available"
    row["candidate"] = {**current, "version": latest, "url": url, "sha256": digest}
    return row


def check(manifest, get=api):
    if manifest.get("schema_version") != 1 or manifest.get("platform") != "linux/amd64":
        raise ValueError("Unsupported toolchain lock schema or platform")
    unknown = set(manifest) - set(SOURCES) - {"schema_version", "platform"}
    if unknown:
        raise ValueError("Unmonitored lock entries: " + ", ".join(sorted(unknown)))
    rows = []
    for name in SOURCES:
        try:
            rows.append(inspect(name, manifest[name], get))
        except (KeyError, ValueError, RuntimeError, OSError) as error:
            rows.append({"tool": name, "current": manifest.get(name, {}).get("version", "missing"),
                         "latest": "unknown", "status": "error", "error": str(error), "candidate": None})
    return {"checked_at": datetime.now(timezone.utc).isoformat(),
            "passed": not any(r["status"] in {"error", "checksum_changed"} for r in rows), "tools": rows}


def cell(value):
    return html.escape(str(value)).replace("|", "&#124;").replace("\n", " ").replace("\r", " ")


def markdown(report):
    lines = ["## Toolchain update check", "", "| Tool | Locked | Upstream | Status |", "| --- | --- | --- | --- |"]
    for row in report["tools"]:
        lines.append("| " + " | ".join(cell(row[k]) for k in ("tool", "current", "latest", "status")) + " |")
    lines += ["", "Candidates are proposals only: review source/checksum evidence, synchronize Dockerfile pins, download and verify inputs, then build and run acceptance before merging.",
              "Temurin stays on JDK 25. Missing checksums require manual review. This check does not audit apt/pip/gem packages or the AFL base image.", ""]
    for row in report["tools"]:
        lines.append(f"- **{cell(row['tool'])}**: {cell(row.get('error') or row.get('checksum_source', ''))}")
        if row.get("release_url"):
            lines.append("  Upstream: " + row["release_url"])
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", type=Path, default=ROOT / "workers/toolchain.lock.json")
    parser.add_argument("--output", type=Path, default=Path("toolchain-report"))
    args = parser.parse_args()
    report = check(json.loads(args.lock.read_text(encoding="utf-8")))
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    (args.output / "report.md").write_text(markdown(report), encoding="utf-8")
    proposals = {r["tool"]: r["candidate"] for r in report["tools"] if r.get("candidate")}
    (args.output / "candidates.json").write_text(json.dumps(proposals, indent=2) + "\n", encoding="utf-8")
    for row in report["tools"]:
        print(f"{row['tool']}: {row['current']} -> {row['latest']} ({row['status']})")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
