"""Validate the signed desktop update manifest and every referenced installer."""

import argparse
import base64
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts._lib.versioning import DIGEST, VERSION

PUBLIC_KEY = ROOT / "src/interfaces/desktop/update-public.key"
RELEASES = "https://github.com/mark0-cn/Wukong/releases"
DATA_SCHEMA = 5
RUNTIME_PROTOCOL = 3
PLATFORMS = ("windows-x86_64", "linux-x86_64")
MAX_ARTIFACT_SIZE = 1024 * 1024 * 1024
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")


class VerificationError(ValueError):
    """The release manifest or an installer failed verification."""


def _base64(value, label):
    try:
        return base64.b64decode(value, validate=True)
    except (ValueError, TypeError) as error:
        raise VerificationError(f"invalid {label} base64") from error


def public_key(value):
    """Decode the checked-in key: base64 of a minisign public key file."""
    try:
        lines = _base64(value.strip(), "public key").decode().splitlines()
    except UnicodeDecodeError as error:
        raise VerificationError("public key is not a minisign file") from error
    if len(lines) < 2:
        raise VerificationError("public key is not a minisign file")
    blob = _base64(lines[1], "public key")
    if len(blob) != 42 or blob[0:2] not in (b"Ed", b"ED"):
        raise VerificationError("unsupported public key")
    return blob[2:10], blob[10:42]


def signature(value):
    """Decode the base64 of a minisign signature file."""
    try:
        lines = _base64(value.strip(), "signature").decode().splitlines()
    except UnicodeDecodeError as error:
        raise VerificationError("signature is not a minisign file") from error
    if len(lines) < 4 or not lines[2].startswith("trusted comment: "):
        raise VerificationError("signature is not a minisign file")
    primary = _base64(lines[1], "signature")
    global_signature = _base64(lines[3], "global signature")
    if len(primary) != 74 or len(global_signature) != 64:
        raise VerificationError("unsigned release artifact")
    if primary[0:2] != b"ED":
        # The updater rejects pre-minisign signatures; publication must too.
        raise VerificationError("unsupported signature algorithm")
    return primary, lines[2][len("trusted comment: ") :], global_signature


def _ed25519(public, data, signature_bytes, openssl):
    if openssl is None:
        raise VerificationError("OpenSSL is required to verify release signatures")
    pem = base64.b64encode(ED25519_SPKI_PREFIX + public).decode()
    lines = "\n".join(pem[index : index + 64] for index in range(0, len(pem), 64))
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        (directory / "public.pem").write_text(
            f"-----BEGIN PUBLIC KEY-----\n{lines}\n-----END PUBLIC KEY-----\n",
            encoding="ascii",
        )
        (directory / "data.bin").write_bytes(data)
        (directory / "signature.bin").write_bytes(signature_bytes)
        result = subprocess.run(
            [
                openssl,
                "pkeyutl",
                "-verify",
                "-pubin",
                "-inkey",
                str(directory / "public.pem"),
                "-rawin",
                "-in",
                str(directory / "data.bin"),
                "-sigfile",
                str(directory / "signature.bin"),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    return result.returncode == 0


def verify_signature(data, signature_text, public_key_text, *, openssl="openssl"):
    """Verify a minisign signature exactly as the desktop updater does."""
    key_id, public = public_key(public_key_text)
    primary, trusted, global_signature = signature(signature_text)
    if primary[2:10] != key_id:
        raise VerificationError("signature does not belong to the release key")
    digest = hashlib.blake2b(data, digest_size=64).digest()
    if not _ed25519(public, digest, primary[10:74], openssl):
        raise VerificationError("release signature verification failed")
    if not _ed25519(public, primary[10:74] + trusted.encode(), global_signature, openssl):
        raise VerificationError("release signature verification failed")


def _hexadecimal(value, length):
    return isinstance(value, str) and re.fullmatch(f"[0-9a-f]{{{length}}}", value) is not None


def _artifact_name(version, artifact):
    prefix = f"{RELEASES}/download/v{version}/"
    url = artifact.get("url")
    if not isinstance(url, str) or not url.startswith(prefix):
        raise VerificationError("release artifact must belong to this release")
    filename = url[len(prefix) :]
    if not filename or re.fullmatch(r"[0-9A-Za-z._-]+", filename) is None:
        raise VerificationError("invalid release artifact URL")
    return filename


def validate_manifest(manifest):
    """Check the release contract before any artifact is read."""
    if not isinstance(manifest, dict):
        raise VerificationError("release manifest is not an object")
    version = manifest.get("version")
    if not isinstance(version, str) or VERSION.fullmatch(version) is None:
        raise VerificationError("stable release version required")
    if manifest.get("schema") != 1:
        raise VerificationError("unsupported release format")
    if manifest.get("runtime_protocol") != RUNTIME_PROTOCOL:
        raise VerificationError("unsupported runtime protocol")
    if manifest.get("data_schema") != DATA_SCHEMA:
        raise VerificationError("unsupported data schema")
    if not _hexadecimal(manifest.get("source_revision"), 40):
        raise VerificationError("invalid source revision")
    image = manifest.get("image")
    if not isinstance(image, str) or not image.startswith("hlmark0/wukong@sha256:"):
        raise VerificationError("image must be pinned to the Wukong repository")
    if not DIGEST.fullmatch(image[len("hlmark0/wukong@") :]):
        raise VerificationError("invalid image identity")
    if not isinstance(manifest.get("image_size"), int) or manifest["image_size"] <= 0:
        raise VerificationError("invalid image identity")
    platforms = manifest.get("platforms")
    if not isinstance(platforms, dict):
        raise VerificationError("release is missing a supported platform")
    for platform in PLATFORMS:
        artifact = platforms.get(platform)
        if not isinstance(artifact, dict):
            raise VerificationError("release is missing a supported platform")
        _artifact_name(version, artifact)
        if not _hexadecimal(artifact.get("sha256"), 64):
            raise VerificationError("invalid artifact checksum")
        size = artifact.get("size")
        if not isinstance(size, int) or not 0 < size <= MAX_ARTIFACT_SIZE:
            raise VerificationError("invalid artifact size")
        if not isinstance(artifact.get("signature"), str) or not artifact["signature"]:
            raise VerificationError("unsigned release artifact")


def verify_directory(directory, *, public_key_path=PUBLIC_KEY, openssl="openssl"):
    """Verify latest.json and every declared installer in *directory*."""
    directory = Path(directory)
    key = Path(public_key_path).read_text(encoding="utf-8")
    manifest_bytes = (directory / "latest.json").read_bytes()
    verify_signature(
        manifest_bytes,
        (directory / "latest.json.sig").read_text(encoding="utf-8"),
        key,
        openssl=openssl,
    )
    try:
        manifest = json.loads(manifest_bytes)
    except json.JSONDecodeError as error:
        raise VerificationError("release manifest is not valid JSON") from error
    validate_manifest(manifest)
    for platform in PLATFORMS:
        artifact = manifest["platforms"][platform]
        path = directory / _artifact_name(manifest["version"], artifact)
        if not path.is_file():
            raise VerificationError(f"missing release artifact {path.name}")
        if path.stat().st_size != artifact["size"] or sha256(path) != artifact["sha256"]:
            raise VerificationError(f"release artifact checksum mismatch: {path.name}")
        verify_signature(path.read_bytes(), artifact["signature"], key, openssl=openssl)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    manifest = verify_directory(args.directory)
    print(f"Verified signed desktop release {manifest['version']}")


if __name__ == "__main__":
    main()
