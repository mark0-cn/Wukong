"""Describe the local Docker image and worker template of a package-local build.

A local-release desktop package instead of the online signed release catalog
carries a `local-release.json` resource: the client version, worker protocol and
data schema, the local Docker image tag, its size, the source revision and an
optional absolute path of a locally built Windows worker media template. This
module computes that descriptor from the current checkout and the host's Docker
state; it never embeds the image itself, a private key or a registry credential.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import zipfile

from scripts._lib.repository import ROOT
from scripts._lib.versioning import version, write_json

RESOURCE = "local-release.json"
SCHEMA = 1
DEFAULT_IMAGE = "wukong:local"
DEFAULT_OUTPUT = Path("target/local-release.json")
DEFAULT_TEMPLATE = Path("target/worker-media-template-gui.zip")
PROTOCOL_LIB = Path("workers/protocol/src/lib.rs")
DATA_SCHEMA_MANIFEST = Path("src/application/maintenance/updates/manifest.rs")
BAKE_TARGET = "workbench"
TEMPLATE_ENV = "WUKONG_LOCAL_TEMPLATE"
IMAGE_ENV = "WUKONG_LOCAL_IMAGE"
REVISION = re.compile(r"[0-9a-f]{40}")
IMAGE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/:@+-]{0,511}")


def _constant(path, name):
    """Read one `pub const <name>: u32 = N;` value instead of hardcoding it."""

    text = (ROOT / path).read_text(encoding="utf-8")
    match = re.search(rf"pub const {name}: u32 = ([0-9]+);", text)
    if not match:
        raise ValueError(f"Cannot read {name} from {path}; the source layout changed")
    return int(match.group(1))


def runtime_protocol():
    return _constant(PROTOCOL_LIB, "PROTOCOL")


def data_schema():
    return _constant(DATA_SCHEMA_MANIFEST, "DATA_SCHEMA")


def source_revision():
    try:
        revision = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise ValueError(f"Cannot resolve the source revision: {error}") from error
    if not REVISION.fullmatch(revision):
        raise ValueError(f"Unexpected source revision: {revision}")
    return revision


def image():
    """Local Docker image tag; `WUKONG_LOCAL_IMAGE` overrides the default."""

    value = os.environ.get(IMAGE_ENV) or DEFAULT_IMAGE
    value = value.strip()
    if not IMAGE.fullmatch(value):
        raise ValueError(
            f"Invalid local Docker image tag {value!r}; set {IMAGE_ENV} to a plain image reference"
        )
    return value


def _docker(*arguments):
    try:
        completed = subprocess.run(["docker", *arguments], cwd=ROOT, text=True, capture_output=True)
    except OSError as error:
        raise ValueError(
            "The Docker CLI is required for local-release packaging; install and start "
            f"Docker Desktop, then retry ({error})"
        ) from error
    return completed


def _image_field(image_name, template):
    completed = _docker("image", "inspect", "--format", template, image_name)
    if completed.returncode != 0:
        if "No such image" in completed.stderr:
            return None
        detail = " ".join(completed.stderr.split())[-500:]
        raise ValueError(
            f"Cannot inspect the Docker image {image_name}: {detail}; "
            "ensure the Docker daemon is running and retry"
        )
    return completed.stdout.strip()


def _bake_command(image_name):
    return [
        "docker",
        "buildx",
        "bake",
        "--load",
        "--set",
        f"{BAKE_TARGET}.tags={image_name}",
    ]


def ensure_image(image_name):
    """Require the local image, building the documented Bake target when absent."""

    if _image_field(image_name, "{{.Id}}") is not None:
        return
    command = _bake_command(image_name)
    printable = subprocess.list2cmdline(command)
    print(
        f"Local Docker image {image_name} is not present; building it from docker-bake.hcl: "
        f"{printable}",
        flush=True,
    )
    hint = (
        f"build it from this checkout with `{printable}` "
        "(run `python -m scripts.toolchain.fetch_worker_deps` first on a fresh clone)"
    )
    try:
        subprocess.run(command, cwd=ROOT, check=True)
    except (OSError, subprocess.CalledProcessError) as error:
        raise ValueError(
            f"The local Docker image {image_name} is unavailable; {hint}: {error}"
        ) from error
    if _image_field(image_name, "{{.Id}}") is None:
        raise ValueError(f"The local Docker image build produced no image; {hint}")


def image_identity(image_name):
    """Return `(size, platform)` of an existing local image."""

    ensure_image(image_name)
    platform = _image_field(image_name, "{{.Os}}/{{.Architecture}}")
    if platform != "linux/amd64":
        raise ValueError(
            f"The local Docker image {image_name} is {platform}; the desktop requires linux/amd64"
        )
    raw = _image_field(image_name, "{{.Size}}")
    try:
        size = int(raw)
    except (TypeError, ValueError) as error:
        raise ValueError(f"Cannot read the size of Docker image {image_name}: {raw!r}") from error
    if size <= 0:
        raise ValueError(f"Docker image {image_name} reports no size")
    return size, platform


def template():
    """Absolute path of a local worker media template, or `None` when absent."""

    configured = os.environ.get(TEMPLATE_ENV)
    if configured:
        path = Path(configured)
        if not path.is_file():
            raise ValueError(f"{TEMPLATE_ENV} does not name an existing file: {path}")
    else:
        path = ROOT / DEFAULT_TEMPLATE
        if not path.is_file():
            return None
    path = path.resolve()
    if not zipfile.is_zipfile(path):
        raise ValueError(f"The local worker media template is not a ZIP archive: {path}")
    return path


def descriptor():
    """Compute the complete descriptor for this checkout and Docker host."""

    image_name = image()
    image_size, _platform = image_identity(image_name)
    payload = {
        "schema": SCHEMA,
        "version": version(),
        "runtime_protocol": runtime_protocol(),
        "data_schema": data_schema(),
        "image": image_name,
        "image_size": image_size,
        "built_at": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "source_revision": source_revision(),
    }
    template_path = template()
    if template_path is not None:
        payload["template_path"] = str(template_path)
    return payload


def prepare(destination=DEFAULT_OUTPUT):
    """Write the descriptor consumed by the bundled desktop resource."""

    payload = descriptor()
    target = Path(destination)
    target = target if target.is_absolute() else ROOT / target
    target.parent.mkdir(parents=True, exist_ok=True)
    write_json(target, payload)
    print(
        f"Local-release descriptor: {target} "
        f"(image {payload['image']}, size {payload['image_size']}, "
        f"protocol {payload['runtime_protocol']}, data schema {payload['data_schema']})",
        flush=True,
    )
    return target, payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true", help="Print the descriptor without writing")
    args = parser.parse_args()
    try:
        if args.check:
            print(json.dumps(descriptor(), indent=2))
        else:
            prepare(args.output)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Local-release preparation failed: {error}\n")


if __name__ == "__main__":
    main()
