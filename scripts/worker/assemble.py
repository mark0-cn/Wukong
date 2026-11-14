"""Seal a complete offline Windows worker payload with a controller-owned key."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts._lib.versioning import version
from scripts.worker.layout import prepare
from scripts.worker.notices import collect
from scripts.worker.paths import member, no_links, regular, unique_paths
from scripts.worker.pe import require_static_crt
from scripts.worker.source_identity import identity
from scripts.worker.toolchain import load_lock
from scripts.worker.build_debugger import BINARY as DEBUGGER_BINARY, validate as validate_debugger

BINARIES = ["wukong-worker.exe", "wukong-package.exe"]
SUPPLEMENTAL_LICENSES = {
    ("fixture3-ddmin", "0.1.1"): ROOT / "workers/windows/licenses/fixture3-ddmin-0.1.1/LICENSE",
    ("jsonrpsee-core", "0.26.1"): ROOT / "workers/windows/licenses/jsonrpsee-0.26.1/LICENSE",
    ("jsonrpsee-types", "0.26.1"): ROOT / "workers/windows/licenses/jsonrpsee-0.26.1/LICENSE",
    ("uuid-simd", "0.8.0"): ROOT / "workers/windows/licenses/simd-0.8.0/LICENSE",
    ("vsimd", "0.8.0"): ROOT / "workers/windows/licenses/simd-0.8.0/LICENSE",
}


def native_build(directory):
    directory = no_links(directory)
    with regular(directory / "native-build.json").open("rb") as incoming:
        raw = incoming.read(64 * 1024 + 1)
    if len(raw) > 64 * 1024:
        raise ValueError("Native build metadata exceeds size limit")
    metadata = json.loads(raw)
    current = identity()
    if (metadata.get("schema") != 1 or metadata.get("platform") != "windows-amd64"
            or metadata.get("static_crt_checked") is not True or set(metadata.get("files", {})) != set(BINARIES)
            or current["source_dirty"] or any(metadata.get(key) != value for key, value in current.items())):
        raise ValueError("Native build requires matching clean committed worker sources; rebuild after committing")
    binaries = directory / "x86_64-pc-windows-msvc/release"
    for name in BINARIES:
        path = regular(binaries / name)
        expected = metadata["files"][name]
        if path.stat().st_size != expected["size"] or sha256(path) != expected["sha256"]:
            raise ValueError("Native build binary no longer matches metadata: " + name)
    # Authenticate all recorded binary hashes before executing the controller's
    # just-built inspector, including that inspector's own static-CRT check.
    inspector = binaries / "wukong-package.exe"
    for name in BINARIES:
        if require_static_crt(binaries / name, inspector=inspector) != metadata["files"][name]["imported_libraries"]:
            raise ValueError("Native build binary imports no longer match metadata: " + name)
    probes = metadata.get("selftests", {})
    if probes.get("schema") != 1 or set(probes.get("architectures", {})) != {"x64", "x86"}:
        raise ValueError("Native build is missing offline self-test fixtures")
    for architecture, probe in probes["architectures"].items():
        if (probe.get("binary") != f"selftest/{architecture}/wukong-probe.exe"
                or probe.get("pdb") != f"selftest/{architecture}/wukong-probe.pdb"
                or probe.get("source_sha256") != sha256(ROOT / "workers/windows/fixtures/probe.c")):
            raise ValueError("Self-test fixture does not match the pinned source")
        if (sha256(regular(directory / probe["binary"])) != probe["binary_sha256"]
                or sha256(regular(directory / probe["pdb"])) != probe["pdb_sha256"]
                or require_static_crt(directory / probe["binary"], architecture, inspector) != probe["imported_libraries"]):
            raise ValueError("Offline self-test fixture was changed")
        dependency=probe.get('dependency')
        if dependency is not None:
            if (dependency['binary']!=f'selftest/{architecture}/wukong-fixture.dll'
                    or dependency['source_sha256']!=sha256(ROOT/'workers/windows/fixtures/dependency.c')
                    or sha256(regular(directory/dependency['binary']))!=dependency['sha256']
                    or require_static_crt(directory/dependency['binary'],architecture,inspector)!=dependency['imported_libraries']):
                raise ValueError('Offline launch dependency fixture was changed')
    return binaries, metadata


def file_index(root):
    files = {}
    for path in sorted(root.rglob("*")):
        no_links(path)
        if path.is_dir():
            continue
        path = regular(path)
        name = member(path.relative_to(root).as_posix())
        if name.startswith("originals/"):
            kind = "original_installer"
        elif name.startswith("licenses/") or any(part.endswith(".dist-info") for part in path.parts) and "LICENSE" in path.name.upper():
            kind = "license"
        elif name in {"runtime/python/LICENSE.txt", "runtime/openssh/LICENSE.txt", "runtime/openssh/NOTICE.txt"}:
            kind = "license"
        elif name.startswith(("runtime/helpers/", "runtime/python/Lib/site-packages/wukong_analysis/")):
            kind = "python_source"
        elif name.startswith("runtime/"):
            kind = "runtime"
        else:
            kind = "data"
        files[name] = dict(sha256=sha256(path), size=path.stat().st_size, kind=kind)
    unique_paths(files)
    return files


def assemble(cache, output, native, keys):
    binaries, native_metadata = native_build(native)
    validate_debugger(native, native_metadata.get("debugger"))
    output, keys = no_links(output), no_links(keys)
    if not keys.is_absolute() or keys == output or output in keys.parents:
        raise ValueError("Controller key storage must stay outside the package")
    before = identity()
    prepare(cache, output)
    shutil.copyfile(native / DEBUGGER_BINARY, output / "runtime/sdk/Windows Kits/10/Debuggers/x64/windbg-mcp.exe")
    debugger_notices = []
    for name in native_metadata["debugger"]["files"]:
        if name != DEBUGGER_BINARY:
            is_patch = name.startswith("debugger/patches/")
            destination = (output / "source-patches" / Path(name).name if is_patch else
                           output / "licenses/windbg-mcp-build" / Path(name).relative_to("debugger/licenses"))
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(native / name, destination)
            if not is_patch:
                debugger_notices.append(destination)
    target = output / "runtime/native"
    target.mkdir()
    for name in ["wukong-worker.exe"]:
        shutil.copyfile(binaries / name, target / name)
    for probe in native_metadata["selftests"]["architectures"].values():
        for key in ["binary", "pdb"]:
            target = output / "runtime" / probe[key]
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(native / probe[key], target)
        if probe.get('dependency'):
            target=output/'runtime'/probe['dependency']['binary']
            shutil.copyfile(native/probe['dependency']['binary'],target)
    shutil.copyfile(native / "native-build.json", output / "native-build.json")
    notices = collect(output / "licenses/native", supplemental=SUPPLEMENTAL_LICENSES) + debugger_notices
    lock = load_lock()
    license_paths = {"python": ["runtime/python/LICENSE.txt"], "openssh": ["runtime/openssh/LICENSE.txt", "runtime/openssh/NOTICE.txt"],
                     "debugging_tools": ["licenses/debugging-tools.rtf"],
                     "windbg_mcp": ["licenses/windbg-mcp-LICENSE"]}
    components = {name: dict(component, original_payloads=["originals/" + item["path"] for item in lock["assets"] if item["component"] == name],
                            licenses=license_paths[name]) for name, component in lock["components"].items()}
    manifest = dict(schema=2, platform="windows-amd64", version=version(), source_revision=native_metadata["source_revision"],
        toolchain_lock_sha256=sha256(output / "toolchain.lock.json"), toolchain_lock_path="toolchain.lock.json", runtime_protocol=3,
        minimum_controller_version=version(), entrypoints=dict(worker="runtime/native/wukong-worker.exe", python="runtime/python/python.exe",
            selftest="runtime/helpers/selftest.py", windbg_mcp="runtime/sdk/Windows Kits/10/Debuggers/x64/windbg-mcp.exe",
            dbgeng_library="runtime/sdk/Windows Kits/10/Debuggers/x64/dbgeng.dll", sshd="runtime/openssh/sshd.exe", ssh_keygen="runtime/openssh/ssh-keygen.exe"),
        components=components, license_notices=[path.relative_to(output).as_posix() for path in notices], files=file_index(output))
    if identity() != before:
        raise ValueError("Worker sources changed while assembling the offline payload")
    # The operator draft and protected key never enter the deliverable directory.
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".worker-seal-") as temporary:
        draft = Path(temporary) / "manifest-draft.json"
        draft.write_text(json.dumps(manifest), encoding="utf-8")
        result = subprocess.run([str(binaries / "wukong-package.exe"), "seal", str(output), str(draft), str(keys), version()],
                                check=True, capture_output=True, text=True)
        return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native", type=Path, default=ROOT / "target/windows-worker-native")
    parser.add_argument("--keys", type=Path, required=True, help="Controller-private directory outside output")
    args = parser.parse_args()
    try:
        print(json.dumps(assemble(args.cache, args.output, args.native, args.keys)))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Offline package assembly failed: {error}\n")


if __name__ == "__main__":
    main()
