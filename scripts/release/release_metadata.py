"""Read image identity from the exact version's public GitHub Release assets."""
import hashlib
import json
import re
import urllib.error
import urllib.parse
import urllib.request

from scripts._lib.versioning import validate_tag
from scripts.release.cli import validate_image

DEFAULT_REPOSITORY = "mark0-cn/Wukong"
MAX_METADATA_BYTES = 1024 * 1024


class HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urllib.parse.urlsplit(newurl).scheme != "https":
            raise ValueError("Release asset redirect must use HTTPS")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(url):
    request = urllib.request.Request(url, headers={"User-Agent": "Wukong-local-package"})
    try:
        with urllib.request.build_opener(HttpsRedirect()).open(request, timeout=30) as response:
            content = response.read(MAX_METADATA_BYTES + 1)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            raise ValueError(f"Release asset not found: {url}. Publish this version first, or supply --image-metadata.") from error
        raise ValueError(f"Cannot download {url}: HTTP {error.code}") from error
    except (urllib.error.URLError, TimeoutError) as error:
        raise ValueError(f"Cannot download {url}: check network access to GitHub and retry") from error
    if len(content) > MAX_METADATA_BYTES:
        raise ValueError("Release metadata exceeds 1 MiB")
    return content


def release_image(number, revision, repository=DEFAULT_REPOSITORY):
    tag = "v" + number
    validate_tag(tag)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*", repository):
        raise ValueError("Release repository must be OWNER/REPO on github.com")
    base = f"https://github.com/{repository}/releases/download/{tag}/"
    print(f"Downloading image metadata from {repository} Release {tag}", flush=True)
    # Direct versioned asset URLs avoid the API's anonymous rate limit. Never use latest.
    content = download(base + "release.json")
    sums = download(base + "SHA256SUMS").decode("utf-8-sig")
    matches = [match[1].lower() for line in sums.splitlines()
               if (match := re.fullmatch(r"([a-fA-F0-9]{64}) [ *]release\.json", line))]
    if len(matches) != 1 or hashlib.sha256(content).hexdigest() != matches[0]:
        raise ValueError("Downloaded release.json does not match a unique SHA256SUMS entry")
    manifest = json.loads(content.decode("utf-8-sig"))
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 2:
        raise ValueError("Unsupported Release manifest")
    if manifest.get("version") != number:
        raise ValueError(f"Release manifest version does not match {tag}")
    if manifest.get("platform") != "windows-amd64" or manifest.get("tool_platform") != "linux/amd64":
        raise ValueError("Release must provide a Windows x64 client and linux/amd64 image")
    if manifest.get("runtime_protocol") != 3 or manifest.get("data_schema") != 4:
        raise ValueError("Release runtime protocol or data schema is not supported")
    source = manifest.get("image_source_revision", manifest.get("source_revision"))
    if not isinstance(source, str) or not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("Release must record the image's original source commit")
    reference = manifest.get("image")
    if not isinstance(reference, str) or reference.count("@") != 1:
        raise ValueError("Release image must be pinned by registry digest")
    image_repository, digest = reference.split("@")
    image = dict(repository=image_repository, digest=digest, image_id=manifest.get("image_id"),
                 source_revision=source, release_version=number, platform=manifest["tool_platform"],
                 size_bytes=manifest.get("image_size_bytes"))
    if not isinstance(image["image_id"], str) or type(image["size_bytes"]) is not int:
        raise ValueError("Release is missing image ID or size")
    validate_image(image, revision, released_version=number)
    return image
