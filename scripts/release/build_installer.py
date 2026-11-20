"""Build one native desktop distribution; Windows bundles the offline worker setup.

The default path is the signed/online release path used by CI tooling and docs.
`local_release=True` is the `cargo package-local` path: it bundles a
`local-release.json` descriptor and builds the client with the `local-release`
Cargo feature so first-run onboarding can use a locally built Docker image
without the online signed catalog. CI release builds never set it.
"""

import argparse
import json
import os
import subprocess
from scripts._lib import msvc
from scripts._lib.versioning import ROOT
from scripts.release import worker_setup
from scripts.release.local_release import RESOURCE as LOCAL_RELEASE_RESOURCE
from scripts.release.local_release import prepare as prepare_local_release

WORKER_SETUP_SOURCE = (worker_setup.DEFAULT_OUTPUT / worker_setup.SETUP_NAME).as_posix()
WORKER_SETUP_RESOURCE = "worker-setup/" + worker_setup.SETUP_NAME
LOCAL_RELEASE_SOURCE = "target/" + LOCAL_RELEASE_RESOURCE


def configuration(platform, signed=False, local_release=False):
    """Compose the Tauri config overlay for one platform."""

    bundle = {"createUpdaterArtifacts": signed}
    resources = {}
    if platform == "windows":
        resources[WORKER_SETUP_SOURCE] = WORKER_SETUP_RESOURCE
    if local_release:
        resources[LOCAL_RELEASE_SOURCE] = LOCAL_RELEASE_RESOURCE
    if resources:
        bundle["resources"] = resources
    return json.dumps({"bundle": bundle}, separators=(",", ":"))


def build(platform, signed=False, local_release=False):
    if platform not in ("windows", "linux"):
        raise ValueError("unsupported desktop platform")
    if signed and not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        raise ValueError("TAURI_SIGNING_PRIVATE_KEY is required for signed installers")
    if local_release:
        # Resolve the local Docker image, its size and the optional local worker
        # template before the long native build; the descriptor becomes a Tauri resource.
        prepare_local_release()
    environment = None
    if platform == "windows":
        environment = worker_setup.build_environment()
        msvc.preflight(environment, ROOT)
        worker_setup.build_and_stage(ROOT / "target/worker-setup", environment=environment)
    npm = "npm.cmd" if os.name == "nt" else "npm"
    npx = "npx.cmd" if os.name == "nt" else "npx"
    command = [npx, "--no-install", "tauri", "build", "--ci"]
    if local_release:
        # Embed the frontend exactly like the release path and enable onboarding
        # against the local descriptor.
        command += ["--features", "custom-protocol,local-release"]
    command += ["--config", configuration(platform, signed, local_release), "--", "--locked"]
    subprocess.run([npm, "ci"], cwd=ROOT, env=environment, check=True)
    subprocess.run(command, cwd=ROOT, env=environment, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=["windows", "linux"])
    parser.add_argument("--signed", action="store_true")
    parser.add_argument(
        "--local-release",
        action="store_true",
        help="Bundle local-release.json and enable the local-release Cargo feature",
    )
    args = parser.parse_args()
    build(args.platform, args.signed, args.local_release)
