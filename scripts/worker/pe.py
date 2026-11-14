"""Bootstrap policy over the existing object-backed controller inspector."""
import json
import re
import subprocess

from scripts._lib.repository import ROOT
from scripts.worker.paths import regular


def imported_libraries(path, architecture="x64", inspector=None):
    if architecture not in {"x64", "x86"}:
        raise ValueError("Unsupported Windows PE architecture")
    path = regular(path)
    if path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("Bootstrap PE image exceeds 64 MiB")
    if inspector is None:
        inspector = ROOT / "target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-package.exe"
    inspector = regular(inspector)
    try:
        result = subprocess.run([str(inspector), "inspect-pe", str(path), architecture],
            check=True, capture_output=True, text=True, encoding="utf-8", timeout=15)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise ValueError("Object PE inspection failed") from error
    if len(result.stdout) > 2 * 1024 * 1024:
        raise ValueError("PE inspector response exceeds limit")
    value = json.loads(result.stdout)
    if (not isinstance(value, dict) or set(value) != {"schema", "architecture", "imported_libraries", "source"}
            or value["schema"] != 1 or value["architecture"] != architecture or value["source"] != "object_pe"):
        raise ValueError("PE inspector response binding differs")
    imports = value["imported_libraries"]
    if (not isinstance(imports, list) or not 1 <= len(imports) <= 6554
            or any(not isinstance(name, str) or len(name) > 255
                   or not re.fullmatch(r"[a-z0-9_-]+\.dll", name) for name in imports)
            or imports != sorted(set(imports))):
        raise ValueError("Invalid PE inspector library names")
    return imports


def require_static_crt(path, architecture="x64", inspector=None):
    imports = imported_libraries(path, architecture, inspector)
    forbidden = [name for name in imports if name.startswith(("vcruntime", "msvcp", "msvcr", "ucrtbase", "api-ms-win-crt-"))]
    if forbidden:
        raise ValueError("Offline bootstrap binary requires an external C runtime: " + ", ".join(forbidden))
    return imports
