"""Ask pip to prove dependency closure using only hash-verified offline wheels."""
from email.parser import Parser
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

from scripts._lib.checksums import sha256
from scripts.worker.paths import regular


def verify(archives, requirements, python_version):
    archives = [regular(Path(path)).resolve() for path in archives]
    requirements = regular(Path(requirements)).resolve()
    # This repository's input is a pin list, not pip configuration. Direct
    # references and find-links directives could bypass --no-index.
    for line in requirements.read_text(encoding="utf-8").splitlines():
        pin = line.strip()
        if pin and not pin.startswith("#") and ("==" not in pin or pin.startswith("-")
                or any(char in pin for char in ";/@:\\ \t")):
            raise ValueError("Offline requirements must contain plain version pins")
    expected = {sha256(path) for path in archives}
    if not archives or len(expected) != len(archives):
        raise ValueError("Offline wheel inventory is empty or duplicated")
    for archive in archives:
        with zipfile.ZipFile(archive) as wheel:
            metadata = [item for item in wheel.infolist() if item.filename.endswith(".dist-info/METADATA")]
            if len(metadata) != 1 or metadata[0].file_size > 1024 * 1024:
                raise ValueError("Invalid offline wheel metadata")
            package = Parser().parsestr(wheel.read(metadata[0]).decode("utf-8"))
            # pip's target flags check Requires-Python and wheel compatibility;
            # environment markers can still use the builder's interpreter/OS.
            # This lock uses unconditional dependencies, so reject ambiguity.
            if any(";" in requirement or "@" in requirement for requirement in package.get_all("Requires-Dist", [])):
                raise ValueError("Conditional or direct-reference wheel dependencies are not offline lock inputs")
    with tempfile.TemporaryDirectory(prefix="wukong-wheel-resolution-") as temporary:
        report = Path(temporary) / "report.json"
        result = subprocess.run([
            sys.executable, "-m", "pip", "--isolated", "--disable-pip-version-check",
            "install", "--dry-run", "--ignore-installed", "--no-index", "--no-cache-dir",
            "--only-binary=:all:", "--platform=win_amd64", "--implementation=cp",
            "--python-version=" + python_version, "--report", str(report),
            "-r", str(requirements), *map(str, archives),
        ], capture_output=True, text=True, timeout=120,
            env={**os.environ, "PIP_CONFIG_FILE": os.devnull})
        if result.returncode:
            raise ValueError("Offline wheel dependency closure failed: " + result.stderr[-2048:])
        if report.stat().st_size > 4 * 1024 * 1024:
            raise ValueError("Offline wheel resolution report exceeds bound")
        resolved = json.loads(report.read_text(encoding="utf-8"))
        actual = {item["download_info"]["archive_info"]["hashes"]["sha256"]
                  for item in resolved["install"]}
        if resolved.get("version") != "1" or actual != expected or len(resolved["install"]) != len(archives):
            raise ValueError("Resolver selected wheels outside the verified lock")
    return {"wheels": len(archives), "dependency_closure": True, "network": False}
