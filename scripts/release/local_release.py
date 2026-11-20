"""Describe the local Docker image and worker template of a package-local build.

A local-release desktop package instead of the online signed release catalog
carries a `local-release.json` resource: the client version, worker protocol and
data schema, the local Docker image tag, the version-pinned CI workbench image
first-run onboarding may pull when that tag is absent, the local image size, the
source revision and an optional absolute path of a locally built Windows worker
media template. This module computes that descriptor from the current checkout
and the host's Docker state; it never embeds the image itself, a private key or a
registry credential.

The workbench image is resolved from an explicit `WUKONG_LOCAL_IMAGE`, the
CI-built Docker Hub workbench image for this client version (pulled, verified as
linux/amd64 and tagged locally, replacing any stale local tag), an already
present `wukong:local` tag only when that pull is impossible, or a local
`docker buildx bake` fallback. The registry repository is intentionally separate
from the official release image, so packaging can never pull a release tag by
mistake. The CI image reference is recorded in the descriptor unconditionally,
even when the packaging host cannot reach Docker, so a packaged installer can
obtain it during first-run onboarding on another machine.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys
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
SIZE_ENV = "WUKONG_LOCAL_IMAGE_SIZE"


class DockerUnavailable(ValueError):
    """The packaging process cannot reach the Docker daemon."""


TEMPLATE_ENV = "WUKONG_LOCAL_TEMPLATE"
IMAGE_ENV = "WUKONG_LOCAL_IMAGE"
REGISTRY_ENV = "WUKONG_LOCAL_IMAGE_REGISTRY"
DEFAULT_REGISTRY = "hlmark0/wukong-workbench"
OFFICIAL_IMAGE_NAME = "wukong"
REVISION = re.compile(r"[0-9a-f]{40}")
IMAGE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/:@+-]{0,511}")
REGISTRY = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*(?::[0-9]+)?(?:/[A-Za-z0-9][A-Za-z0-9._-]*)+")


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
    """Local Docker image tag; `WUKONG_LOCAL_IMAGE` overrides the default.

    An explicit override also wins over the registry pull: `ensure_image` builds
    that tag from the documented Bake target instead of pulling and retagging.
    """

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


def _docker_unavailable(stderr):
    """Whether a Docker CLI failure means the daemon is unreachable, not that the
    requested object is missing. Sandboxed shells and missing user permissions
    surface this as a named-pipe or API connection denial; the package can still
    be built when onboarding verifies the image on the real Docker host."""

    text = (stderr or "").lower()
    return any(
        marker in text
        for marker in (
            "permission denied",
            "cannot connect",
            "error during connect",
            "is the docker daemon running",
            "dockerdesktoplinuxengine",
            "npipe:",
        )
    )


def _image_field(image_name, template):
    completed = _docker("image", "inspect", "--format", template, image_name)
    if completed.returncode != 0:
        if "No such image" in completed.stderr:
            return None
        detail = " ".join(completed.stderr.split())[-500:]
        if _docker_unavailable(completed.stderr):
            raise DockerUnavailable(detail)
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


def registry():
    """Docker Hub repository of the CI-built workbench image.

    It is deliberately separate from the official release image repository: a
    local package may pull a workbench image, but never an official release tag.
    """

    value = (os.environ.get(REGISTRY_ENV) or DEFAULT_REGISTRY).strip()
    if not REGISTRY.fullmatch(value):
        raise ValueError(
            f"Invalid {REGISTRY_ENV} {value!r}; set it to a Docker Hub repository "
            f"such as {DEFAULT_REGISTRY}"
        )
    if value.rsplit("/", 1)[-1] == OFFICIAL_IMAGE_NAME:
        raise ValueError(
            f"{REGISTRY_ENV} must name a separate workbench repository, not the "
            f"official release image repository {value!r}"
        )
    return value


def workbench_image():
    """Immutable CI-built image reference for this client version."""

    return f"{registry()}:local-{version()}"


def _pull_failure(stderr):
    """Classify a pull failure that a local image build can still recover from.

    Authorization, missing-image and network failures fall back to the Bake
    path. Other failures (for example a published image without an amd64
    manifest) keep their error so a broken publication is never hidden.
    """

    text = (stderr or "").lower()
    if any(
        marker in text for marker in ("manifest unknown", "not found", "repository does not exist")
    ):
        return "not found"
    if any(
        marker in text
        for marker in (
            "unauthorized",
            "authentication required",
            "no basic auth",
            "pull access denied",
            "denied",
        )
    ):
        return "not authorized"
    if any(
        marker in text
        for marker in (
            "connection",
            "timed out",
            "timeout",
            "no such host",
            "network",
            "tls",
            "eof",
        )
    ):
        return "network"
    return None


def _warn_pull_fallback(reference, reason, detail, image_name):
    command = subprocess.list2cmdline(_bake_command(image_name))
    print(
        f"warning: cannot pull the workbench image {reference} ({reason}: {detail}); "
        "ask a maintainer to run the Workbench image workflow for this version "
        "(Actions -> Workbench image -> Run workflow), or build it locally with "
        f"`{command}`; if the Docker Hub repository is private, run `docker login` first",
        file=sys.stderr,
        flush=True,
    )


def _warn_existing_image(image_name):
    """Warn that the pull failed and an older local tag is used instead."""

    reference = workbench_image()
    print(
        f"warning: falling back to the existing local Docker image {image_name}, which "
        f"may be an older build than the CI image {reference}; remove it with "
        f"`docker image rm {image_name}` and re-run to force the pull",
        file=sys.stderr,
        flush=True,
    )


def pull_workbench_image(image_name):
    """Pull the CI-built image for this version, verify it and tag it locally.

    Returns `True` when the local tag now exists. Missing-image, authorization
    and network failures only warn because the caller can still fall back to an
    existing local tag or the local Bake path; a non-linux/amd64 image is refused
    instead of silently used.
    """

    reference = workbench_image()
    completed = _docker("pull", "--platform", "linux/amd64", reference)
    if completed.returncode != 0:
        detail = " ".join(completed.stderr.split())[-500:]
        if _docker_unavailable(completed.stderr):
            raise DockerUnavailable(detail)
        reason = _pull_failure(completed.stderr)
        if reason is None:
            raise ValueError(
                f"Cannot pull the CI-built workbench image {reference}: {detail}; "
                "ensure the Docker daemon can reach Docker Hub, the version tag is "
                "published and the host has free disk space, then retry"
            )
        _warn_pull_fallback(reference, reason, detail, image_name)
        return False
    platform = _image_field(reference, "{{.Os}}/{{.Architecture}}")
    if platform != "linux/amd64":
        raise ValueError(
            f"The CI-built workbench image {reference} is {platform}; "
            "the desktop requires linux/amd64"
        )
    completed = _docker("tag", reference, image_name)
    if completed.returncode != 0:
        detail = " ".join(completed.stderr.split())[-500:]
        raise ValueError(f"Cannot tag {reference} as {image_name}: {detail}")
    if _image_field(image_name, "{{.Id}}") is None:
        raise ValueError(f"Tagging {reference} as {image_name} produced no image")
    print(
        f"Using the CI-built workbench image {reference} (linux/amd64) as {image_name}",
        flush=True,
    )
    return True


def _explicit_image():
    """Whether `WUKONG_LOCAL_IMAGE` names the image explicitly."""

    return bool((os.environ.get(IMAGE_ENV) or "").strip())


def _bake_image(image_name):
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


def _ensure_image(image_name):
    """Resolve the image in the documented priority.

    Raises `DockerUnavailable` when neither the pull nor the local check can
    reach the daemon, so `ensure_image` can defer verification to onboarding.
    """

    if _explicit_image():
        if _image_field(image_name, "{{.Id}}") is None:
            _bake_image(image_name)
        return
    if pull_workbench_image(image_name):
        return
    if _image_field(image_name, "{{.Id}}") is not None:
        _warn_existing_image(image_name)
        return
    _bake_image(image_name)


def ensure_image(image_name):
    """Require the local image, preferring the CI-built workbench image.

    Resolution order: an explicit `WUKONG_LOCAL_IMAGE`; the CI-built Docker Hub
    image for this client version (`WUKONG_LOCAL_IMAGE_REGISTRY`, default
    `hlmark0/wukong-workbench`, tag `local-<version>`) pulled, verified as
    linux/amd64 and tagged as `wukong:local`, which replaces an older local tag;
    an already present `wukong:local` tag, only when that pull is impossible;
    and finally the documented Bake target. When the packaging process cannot
    reach Docker, verification is deferred to onboarding instead of failing the
    build; onboarding requires the image and reads its real size before
    preparing the environment.
    """

    try:
        _ensure_image(image_name)
    except DockerUnavailable as error:
        print(
            f"warning: Docker is not reachable from this packaging process; "
            f"recording {image_name} without a size and deferring verification to "
            f"onboarding ({error})",
            file=sys.stderr,
            flush=True,
        )


def image_identity(image_name):
    """Return `(size, platform)` of an existing local image.

    A packaging process without Docker access records size 0 ("unknown") and
    defers the check to onboarding; `WUKONG_LOCAL_IMAGE_SIZE` records it
    explicitly when the image is known to be built already.
    """

    try:
        ensure_image(image_name)
        platform = _image_field(image_name, "{{.Os}}/{{.Architecture}}")
        if platform != "linux/amd64":
            raise ValueError(
                f"The local Docker image {image_name} is {platform}; the desktop requires linux/amd64"
            )
        raw = _image_field(image_name, "{{.Size}}")
    except DockerUnavailable:
        configured = os.environ.get(SIZE_ENV)
        if configured:
            try:
                size = int(configured)
            except ValueError as error:
                raise ValueError(
                    f"{SIZE_ENV} must be a positive integer: {configured!r}"
                ) from error
            if size <= 0:
                raise ValueError(f"{SIZE_ENV} must be a positive integer: {configured!r}")
            print(
                f"warning: Docker is not reachable; recording {SIZE_ENV}={size} "
                f"for {image_name} and deferring platform verification to onboarding",
                file=sys.stderr,
                flush=True,
            )
            return size, "linux/amd64"
        print(
            f"warning: Docker is not reachable; recording {image_name} without a "
            "size; onboarding verifies the image on the Docker host",
            file=sys.stderr,
            flush=True,
        )
        return 0, "unknown"
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
        # Recorded even when the packaging host cannot reach Docker: onboarding
        # on another machine pulls and tags it when the local image is absent.
        "image_registry": workbench_image(),
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
        f"(image {payload['image']}, registry {payload['image_registry']}, "
        f"size {payload['image_size']}, "
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
