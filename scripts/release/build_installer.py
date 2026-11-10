"""Build one native desktop distribution; tool images are downloaded on first use."""
import argparse
import os
import subprocess
from scripts._lib.versioning import ROOT


def build(platform, signed=False):
    if platform not in ("windows", "linux"):
        raise ValueError("unsupported desktop platform")
    if signed and not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        raise ValueError("TAURI_SIGNING_PRIVATE_KEY is required for signed installers")
    npm = "npm.cmd" if os.name == "nt" else "npm"
    npx = "npx.cmd" if os.name == "nt" else "npx"
    subprocess.run([npm, "ci"], cwd=ROOT, check=True)
    subprocess.run([npx, "--no-install", "tauri", "build", "--ci", "--config",
                    '{"bundle":{"createUpdaterArtifacts":' + str(signed).lower() + '}}', '--', '--locked'],
                   cwd=ROOT, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=["windows", "linux"])
    parser.add_argument("--signed", action="store_true")
    args = parser.parse_args()
    build(args.platform, args.signed)
