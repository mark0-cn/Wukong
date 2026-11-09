"""Promote an accepted image and finish a retryable GitHub Release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from release import read_json, sha256, version


def run(argv, **kwargs):
    return subprocess.check_output(argv, text=True, **kwargs).strip()


def image_digest(reference, missing_ok=False):
    result = subprocess.run(["docker", "buildx", "imagetools", "inspect", reference], capture_output=True, text=True)
    if result.returncode:
        if missing_ok and re.search(r"not found|manifest unknown", result.stderr, re.I):
            return None
        raise RuntimeError(result.stderr)
    match = re.search(r"^Digest:\s+(sha256:[0-9a-f]{64})", result.stdout, re.M)
    if not match:
        raise ValueError("Registry did not return an image digest")
    return match[1]


def promote(repository, digest, tag):
    reference = repository + ":" + tag
    existing = image_digest(reference, missing_ok=True)
    if existing == digest:
        return
    if existing and tag != "latest":
        raise ValueError("Refusing to overwrite published image " + reference)
    run(["docker", "buildx", "imagetools", "create", "--tag", reference, repository + "@" + digest])
    if image_digest(reference) != digest:
        raise ValueError("Promotion changed the accepted image index")


def releases(repository):
    return json.loads(run(["gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases?per_page=100"]))


def publish(directory, tag):
    number = version(tag)
    repository = os.environ["GITHUB_REPOSITORY"]
    manifest = read_json(directory / "release.json")
    validation = read_json(directory / "validation-summary.json")
    revision = run(["git", "rev-parse", "HEAD"])
    if manifest["source_revision"] != revision or manifest["version"] != number:
        raise ValueError("Release identity mismatch")
    if not validation.get("passed") or validation["source_revision"] != revision or validation["image"] != manifest["image"]:
        raise ValueError("Candidate did not pass acceptance")
    # Check the complete final asset set before any registry/release mutation.
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        checksum, name = line.split("  ", 1)
        if Path(name).name != name or sha256(directory / name) != checksum:
            raise ValueError("Asset checksum mismatch: " + name)
    repo, digest = manifest["image"].split("@")
    all_releases = [r for page in releases(repository) for r in page]
    existing = next((r for r in all_releases if r["tag_name"] == tag), None)
    if existing and not existing["draft"]:
        # A published release is never replaced; retries only complete promotion.
        with tempfile.TemporaryDirectory() as folder:
            run(["gh", "release", "download", tag, "--repo", repository, "--dir", folder])
            for asset in directory.iterdir():
                downloaded = Path(folder) / asset.name
                if not downloaded.is_file() or sha256(downloaded) != sha256(asset):
                    raise ValueError("Published assets differ; use a new version")
    else:
        notes = directory.parent / "release-notes.md"
        generated = json.loads(run(["gh", "api", f"repos/{repository}/releases/generate-notes", "-f", "tag_name=" + tag, "-f", "target_commitish=" + revision]))
        notes.write_text(generated["body"] + f'\n\n## Online installation\n\nPrivate tool image: `{manifest["image"]}`. Authenticate to Docker Hub before installation. Download the Windows zip, extract it, then run `install-workbench.ps1`.\n\nChecks: Linux tool/container/debugger acceptance passed; Windows Docker Desktop end-to-end is not covered by hosted runners.\n', encoding="utf-8")
        if not existing:
            run(["gh", "release", "create", tag, "--repo", repository, "--verify-tag", "--draft", "--title", tag, "--notes-file", str(notes), "--target", revision])
        run(["gh", "release", "upload", tag, "--repo", repository, "--clobber", *map(str, sorted(directory.iterdir()))])
        promote(repo, digest, number)
        run(["gh", "release", "edit", tag, "--repo", repository, "--draft=false", "--prerelease=" + str("-" in number).lower(), "--latest=false", "--notes-file", str(notes)])
    promote(repo, digest, number)
    if "-" not in number:
        current = tuple(map(int, number.split(".")))
        published = [r for page in releases(repository) for r in page]
        newer = any(not r["draft"] and not r["prerelease"] and re.fullmatch(r"v\d+\.\d+\.\d+", r["tag_name"]) and tuple(map(int, r["tag_name"][1:].split("."))) > current for r in published)
        if not newer:
            promote(repo, digest, "latest")
            run(["gh", "release", "edit", tag, "--repo", repository, "--latest"])
    verified = json.loads(run(["gh", "release", "view", tag, "--repo", repository, "--json", "url,isDraft,assets"]))
    if verified["isDraft"] or {p.name for p in directory.iterdir()} != {a["name"] for a in verified["assets"]}:
        raise ValueError("Published release assets are incomplete")
    print(verified["url"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--tag")
    parser.add_argument("--inspect-image")
    args = parser.parse_args()
    if args.inspect_image:
        print(image_digest(args.inspect_image, missing_ok=True) or "")
    else:
        publish(args.directory, args.tag)
