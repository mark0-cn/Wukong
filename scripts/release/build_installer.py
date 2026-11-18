"""Build one native desktop distribution; Windows bundles the offline worker setup."""

import argparse
import json
import os
import subprocess
from scripts._lib.versioning import ROOT
from scripts.release import worker_setup

WORKER_SETUP_SOURCE = (worker_setup.DEFAULT_OUTPUT / worker_setup.SETUP_NAME).as_posix()
WORKER_SETUP_RESOURCE = "worker-setup/" + worker_setup.SETUP_NAME


def configuration(platform, signed=False):
    """Compose the Tauri config overlay for one platform."""

    bundle = {"createUpdaterArtifacts": signed}
    if platform == "windows":
        bundle["resources"] = {WORKER_SETUP_SOURCE: WORKER_SETUP_RESOURCE}
    return json.dumps({"bundle": bundle}, separators=(",", ":"))


def build(platform, signed=False):
    if platform not in ("windows", "linux"):
        raise ValueError("unsupported desktop platform")
    if signed and not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        raise ValueError("TAURI_SIGNING_PRIVATE_KEY is required for signed installers")
    if platform == "windows":
        worker_setup.build_and_stage(ROOT / "target/worker-setup")
    npm = "npm.cmd" if os.name == "nt" else "npm"
    npx = "npx.cmd" if os.name == "nt" else "npx"
    subprocess.run([npm, "ci"], cwd=ROOT, check=True)
    subprocess.run(
        [
            npx,
            "--no-install",
            "tauri",
            "build",
            "--ci",
            "--config",
            configuration(platform, signed),
            "--",
            "--locked",
        ],
        cwd=ROOT,
        check=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=["windows", "linux"])
    parser.add_argument("--signed", action="store_true")
    args = parser.parse_args()
    build(args.platform, args.signed)
