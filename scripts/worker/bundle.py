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


def prepare(package, enrollment, output, key, controller, verifier, previous_package=None, previous_enrollment=None):
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
    if (previous_package is None) != (previous_enrollment is None):
        raise ValueError("Recovery media requires both previous public sources")
    if previous_package is not None:
        previous_package, previous_enrollment = map(no_links, [previous_package, previous_enrollment])
        if (not previous_package.is_absolute() or not previous_enrollment.is_absolute()
                or set(path.name for path in previous_enrollment.iterdir()) != {"enrollment.json", "enrollment.sig"}):
            raise ValueError("Recovery requires absolute public source directories")
        subprocess.run([str(verifier), "verify-enrollment", str(previous_package), str(previous_enrollment), key, controller],
                       check=True, capture_output=True)
        prior = json.loads(regular(previous_enrollment / "enrollment.json").read_text(encoding="utf-8"))
        current = json.loads(regular(enrollment / "enrollment.json").read_text(encoding="utf-8"))
        if (any(current[field] != prior[field] for field in ["installation_id", "environment_id", "controller_public_key"])
                or current["generation"] <= prior["generation"]):
            raise ValueError("Recovery must retain authority/identity and increase the signed generation")
    manifest = json.loads(regular(package / "manifest.json").read_text(encoding="utf-8"))
    worker = regular(package / manifest["entrypoints"]["worker"])
    if sha256(worker) != manifest["files"][manifest["entrypoints"]["worker"]]["sha256"]:
        raise ValueError("Offline setup executable changed")
    output.mkdir()
    shutil.copytree(package, output / "payload")
    (output / "enrollment").mkdir()
    for name in ["enrollment.json", "enrollment.sig"]:
        shutil.copyfile(regular(enrollment / name), output / "enrollment" / name)
    if previous_package is not None:
        (output / "recovery").mkdir()
        for source, names in [(previous_package, ["manifest.json", "manifest.sig"]),
                              (previous_enrollment, ["enrollment.json", "enrollment.sig"])]:
            for name in names:
                shutil.copyfile(regular(source / name), output / "recovery" / name)
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
    parser.add_argument("--previous-package", type=Path)
    parser.add_argument("--previous-enrollment", type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.package.resolve(), args.enrollment.resolve(), args.output.absolute(),
                                 args.public_key, args.controller_version, args.verifier.resolve(),
                                 args.previous_package.resolve() if args.previous_package else None,
                                 args.previous_enrollment.resolve() if args.previous_enrollment else None)))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Offline setup media preparation failed: {error}\n")


if __name__ == "__main__":
    main()
