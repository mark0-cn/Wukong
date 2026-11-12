"""Windows image/symbol correspondence policy over object and SDK observations.

Binary formats are read by the native helper, never by a parallel Python parser.
Embedded producer paths are descriptive data, never file/search capabilities.
"""
from pathlib import Path, PureWindowsPath
import re

from worker_common import sha256_file


def rsds(value):
    return value if value and value.get("format") == "RSDS" else None


def match_module(image, modules, name, codeview_available=False):
    if not isinstance(modules, list) or not 1 <= len(modules) <= 4096:
        raise ValueError("debugger has no bounded module observations")
    named = [row for row in modules if PureWindowsPath(str(row.get("image", ""))).name.casefold() == name.casefold()]
    candidates = []
    for module in named or modules:
        if module.get("size") != image["size"] or module.get("timestamp") != image["timestamp"]:
            continue
        if image["checksum"] and module.get("checksum") and module["checksum"] != image["checksum"]:
            continue
        expected, observed = rsds(image["codeview"]), rsds(module.get("codeview"))
        if expected and observed and (expected["guid"], expected["age"]) != (observed["guid"], observed["age"]):
            continue
        candidates.append(module)
    if len(candidates) != 1:
        raise ValueError("staged PE does not uniquely correspond to the dump/debugger module metadata")
    module = candidates[0]
    codeview_matched = bool(codeview_available and rsds(image["codeview"]) and rsds(module.get("codeview")))
    return {"scope":"module_metadata_and_rsds" if codeview_matched else "module_metadata_only",
            "base":module["base"], "image":PureWindowsPath(str(module.get("image", ""))).name[:256],
            "checksum_compared":bool(image["checksum"] and module.get("checksum")),
            "codeview_matched":codeview_matched, "byte_identity_verified":False}


def local_symbols(image, directory, validate, probe):
    expected = rsds(image["codeview"])
    if not expected:
        return {"status":"no_supported_rsds_identity", "pdb_identity_matched":False}
    name = expected["pdb_name"]
    # A CodeView name is never an arbitrary file capability. Only a simple
    # candidate directly in the explicitly configured local symbol directory.
    if (not name or len(name) > 255 or name.endswith((".", " "))
            or re.match(r"^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)", name, re.IGNORECASE)
            or any(c in name for c in '<>:"/\\|?*\0') or any(ord(c) < 32 for c in name)):
        return {"status":"unusable_pdb_name", "pdb_identity_matched":False}
    candidate = Path(directory) / name
    if not candidate.exists():
        return {"status":"missing", "pdb_name":name, "guid":expected["guid"], "age":expected["age"], "pdb_identity_matched":False}
    candidate = validate(candidate)
    observed = probe(candidate)
    if observed["file_sha256"] != sha256_file(candidate):
        raise ValueError("local PDB changed during SDK identity inspection")
    if (observed["guid"], observed["age"]) != (expected["guid"], expected["age"]):
        raise ValueError("local PDB GUID/age does not match the staged PE CodeView identity")
    return {"status":"matched_rsds", **observed, "pdb_name":name,
            "pdb_sha256":observed["file_sha256"], "pdb_identity_matched":True}
