"""Prepare public offline VM setup media from a sealed payload and enrollment.

This controller command neither installs anything nor copies private keys. The
native VM setup shows the controller fingerprint for independent first trust.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

from scripts._lib.checksums import sha256
from scripts.worker.paths import no_links, regular


def prepare(package, enrollment, output, key, controller, verifier):
    package, enrollment, output = map(no_links, [package, enrollment, output])
    if not all(path.is_absolute() for path in [package, enrollment, output, verifier]):
        raise ValueError("Offline media paths must be absolute")
    if output.exists() or any(source == output or source in output.parents or output in source.parents
                              for source in [package, enrollment]):
        raise ValueError("Offline media needs a new directory separate from its sources")
    if set(path.name for path in enrollment.iterdir()) != {"enrollment.json", "enrollment.sig"}:
        raise ValueError("Enrollment source must contain only the two public signed records")
    result = subprocess.run([str(regular(verifier)), "verify-enrollment", str(package), str(enrollment), key, controller],
                            check=True, capture_output=True, text=True)
    verified = json.loads(result.stdout)
    manifest = json.loads(regular(package / "manifest.json").read_text(encoding="utf-8"))
    worker = regular(package / manifest["entrypoints"]["worker"])
    if sha256(worker) != manifest["files"][manifest["entrypoints"]["worker"]]["sha256"]:
        raise ValueError("Offline setup executable changed")
    output.mkdir()
    shutil.copytree(package, output / "payload")
    (output / "enrollment").mkdir()
    for name in ["enrollment.json", "enrollment.sig"]:
        shutil.copyfile(regular(enrollment / name), output / "enrollment" / name)
    shutil.copyfile(worker, output / "WukongWorkerSetup.exe")
    (output / "setup.json").write_text(json.dumps(dict(schema=1, controller_public_key=key,
        controller_version=controller), indent=2) + "\n", encoding="utf-8")
    # Verify the copied bytes too; source verification cannot prove the media.
    subprocess.run([str(verifier), "verify-enrollment", str(output / "payload"), str(output / "enrollment"), key, controller],
                   check=True, capture_output=True)
    return dict(prepared=True, scope="public_offline_setup_media", installation_id=verified["installation_id"],
                manifest_sha256=verified["manifest_sha256"], setup_sha256=sha256(output / "WukongWorkerSetup.exe"),
                installed=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--enrollment", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--public-key", required=True)
    parser.add_argument("--controller-version", required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.package.resolve(), args.enrollment.resolve(), args.output.absolute(),
                                 args.public_key, args.controller_version, args.verifier.resolve())))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Offline setup media preparation failed: {error}\n")


if __name__ == "__main__":
    main()
