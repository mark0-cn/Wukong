"""Prepare a complete private Windows runtime from the verified offline cache."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import stat
import tempfile
import xml.etree.ElementTree as ET
import zipfile

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts.worker.msi_layout import extract_cabinet, layout as msi_layout
from scripts.worker.paths import member, no_links, regular, unique_paths
from scripts.worker.toolchain import LOCK, acquire, load_lock, validate_asset


def unpack_zip(archive, output, prefix="", wheel=False):
    output = no_links(output)
    with zipfile.ZipFile(regular(archive)) as source:
        entries = []
        for entry in source.infolist():
            name = entry.filename
            if prefix:
                if not name.startswith(prefix):
                    raise ValueError("Unexpected archive root")
                name = name[len(prefix):]
            if not name:
                continue
            name = member(name.rstrip("/") if entry.is_dir() else name)
            mode = entry.external_attr >> 16
            if (stat.S_ISLNK(mode) or entry.flag_bits & 1
                    or stat.S_IFMT(mode) not in {0, stat.S_IFREG, stat.S_IFDIR}
                    or entry.file_size > 2 * 1024**3):
                raise ValueError("Unsafe archive member")
            if wheel and (".data/" in name or name.lower().endswith(".pth")):
                raise ValueError("Offline wheels must be self-contained pure Python libraries")
            entries.append((entry, name))
        if not entries or len(entries) > 20_000 or sum(e.file_size for e, _ in entries) > 2 * 1024**3:
            raise ValueError("Archive exceeds offline layout budget")
        unique_paths(name for entry, name in entries if not entry.is_dir())
        files = {name.lower() for entry, name in entries if not entry.is_dir()}
        for entry, name in entries:
            if entry.is_dir() and name.lower() in files:
                raise ValueError("Archive file/directory collision")
        # Explicit directory aliases cannot change the spelling of a file's ancestors.
        names = {}
        for entry, name in entries:
            current = ""
            for part in name.split("/"):
                current = "/".join(filter(None, [current, part]))
                folded = current.lower()
                if folded in names and names[folded] != current:
                    raise ValueError("Case-colliding archive directories")
                names[folded] = current
        if wheel:
            wheel_metadata = [entry for entry, name in entries if name.endswith(".dist-info/WHEEL")]
            package_metadata = [entry for entry, name in entries if name.endswith(".dist-info/METADATA")]
            if len(wheel_metadata) != 1 or len(package_metadata) != 1:
                raise ValueError("Invalid pure Python wheel metadata")
            if any(entry.file_size > 1024 * 1024 for entry in [*wheel_metadata, *package_metadata]):
                raise ValueError("Wheel metadata exceeds the offline size limit")
            metadata = source.read(wheel_metadata[0]).decode("utf-8")
            package = source.read(package_metadata[0]).decode("utf-8")
            if ("Root-Is-Purelib: true" not in metadata or "Tag: py3-none-any" not in metadata
                    or any(line.startswith("Requires-Dist:") for line in package.splitlines())):
                raise ValueError("Wheel dependency closure is not supported by this lock")
        output.mkdir(parents=True, exist_ok=True)
        for entry, name in entries:
            target = no_links(output / name)
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(entry) as incoming, target.open("xb") as outgoing:
                size = 0
                while chunk := incoming.read(1024 * 1024):
                    size += len(chunk)
                    if size > entry.file_size:
                        raise ValueError("Archive member exceeds declared size")
                    outgoing.write(chunk)
                if size != entry.file_size:
                    raise ValueError("Truncated archive member")


def sdk_metadata(cache, lock, scratch):
    specification = lock["sdk"]
    assets = {item["path"]: item for item in lock["assets"]}
    bootstrapper = validate_asset(cache, assets[specification["bootstrapper"]])
    offset, size = specification["ux_offset"], specification["ux_size"]
    if (type(offset) is not int or type(size) is not int or not 0 < size < 16 * 1024**2
            or offset < 0 or offset + size > bootstrapper.stat().st_size):
        raise ValueError("Invalid SDK bootstrapper cabinet range")
    cabinet = scratch / "sdk-ux.cab"
    with bootstrapper.open("rb") as incoming, cabinet.open("xb") as outgoing:
        incoming.seek(offset)
        payload = incoming.read(size)
        if len(payload) != size or payload[:4] != b"MSCF":
            raise ValueError("SDK bootstrapper cabinet is missing")
        outgoing.write(payload)
    ux = scratch / "sdk-ux"
    extract_cabinet(cabinet, ux, {name: item["size"] for name, item in specification["ux_files"].items()})
    for name, item in specification["ux_files"].items():
        if sha256(ux / member(name)) != item["sha256"]:
            raise ValueError("SDK bootstrapper metadata differs from lock")
    tree = ET.parse(ux / member(specification["manifest"])).getroot()
    namespace = {"b": "http://schemas.microsoft.com/wix/2008/Burn"}
    payloads = {item.attrib["Id"]: item.attrib for item in tree.findall("b:Payload", namespace)}
    packages = [item for item in tree.findall(".//b:MsiPackage", namespace)
                if item.get("Id") == specification["package_id"]]
    if len(packages) != 1:
        raise ValueError("SDK debugger package missing from signed bootstrapper")
    selected = [payloads[item.attrib["Id"]] for item in packages[0].findall("b:PayloadRef", namespace)]
    actual = {}
    for item in selected:
        name = member("sdk/" + item["FilePath"].replace("\\", "/"))
        if name in actual:
            raise ValueError("Duplicate SDK bootstrapper payload")
        actual[name] = (int(item["FileSize"]), item["Hash"].lower())
    expected = {item["path"]: (item["size"], item["vendor_sha1"])
                for item in lock["assets"] if item["path"].startswith("sdk/")}
    if actual != expected:
        raise ValueError("Offline SDK inventory is incomplete or differs from signed bootstrapper")
    for name, (_, digest) in actual.items():
        with regular(cache / name).open("rb") as incoming:
            if hashlib.file_digest(incoming, "sha1").hexdigest() != digest:
                raise ValueError("SDK payload differs from signed bootstrapper identity")
    return ux / member(specification["license"])


def prepare(cache, output, lock_path=LOCK):
    lock = load_lock(lock_path)
    acquire(cache, lock)  # read/validate only; this entry has no download option
    output = no_links(output)
    output.mkdir(parents=True)  # never overwrite another package or failed attempt
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".worker-layout-") as temporary:
        scratch = no_links(temporary)
        license_path = sdk_metadata(cache, lock, scratch)
        licenses = output / "licenses"
        licenses.mkdir()
        shutil.copyfile(license_path, licenses / "debugging-tools.rtf")
        for item in lock["assets"]:
            target = output / "originals" / item["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(validate_asset(cache, item), target)
        python_asset = next(item for item in lock["assets"] if item["component"] == "python")
        ssh_asset = next(item for item in lock["assets"] if item["component"] == "openssh")
        runtime = output / "runtime"
        unpack_zip(cache / python_asset["path"], runtime / "python")
        unpack_zip(cache / ssh_asset["path"], runtime / "openssh", prefix="OpenSSH-Win64/")
        for item in lock["assets"]:
            if item["component"] == "worker_python_dependencies":
                unpack_zip(cache / item["path"], runtime / "python/Lib/site-packages", wheel=True)
        # Embeddable Python ignores registry/environment paths. Keep site disabled,
        # adding only the shipped worker and the already unpacked offline wheels.
        path_files = list((runtime / "python").glob("python*._pth"))
        if len(path_files) != 1:
            raise ValueError("Embedded Python path configuration is missing")
        path_files[0].write_bytes(b"python313.zip\n.\nLib/site-packages\n../worker\n")
        (runtime / "worker").mkdir()
        for path in sorted((ROOT / "workers/runtime").glob("*.py")):
            shutil.copyfile(regular(path), runtime / "worker" / member(path.name))
        shutil.copyfile(ROOT / "workers/runtime/requirements.txt", runtime / "worker/requirements.txt")
        files = msi_layout(cache / lock["sdk"]["database"], cache / "sdk/Installers",
                           runtime / "sdk", scratch / "sdk-files")
        debugger_asset = next(item for item in lock["assets"] if item["component"] == "windbg_mcp")
        debugger = scratch / "windbg-mcp"
        unpack_zip(cache / debugger_asset["path"], debugger)
        # The x64 server handles both x64 and WOW64 targets. Its adjacent engine
        # and CRT are pinned payloads, not dependencies of the guest machine.
        engine = runtime / "sdk/Windows Kits/10/Debuggers/x64"
        for source, name in [(debugger / "windbg-mcp.exe", "windbg-mcp.exe"),
                             (runtime / "python/vcruntime140.dll", "vcruntime140.dll")]:
            with regular(source).open("rb") as incoming, (engine / name).open("xb") as outgoing:
                shutil.copyfileobj(incoming, outgoing)
        shutil.copyfile(regular(debugger / "LICENSE"), licenses / "windbg-mcp-LICENSE")
        shutil.copyfile(regular(lock_path), output / "toolchain.lock.json")
        index = {"schema": 1, "scope": "unsigned_controller_layout", "toolchain_lock_sha256": sha256(lock_path),
                 "sdk_files": len(files), "sdk_bytes": sum(item["size"] for item in files),
                 "components": lock["components"]}
        (output / "layout-index.json").write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")
    return index


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.cache, args.output)))
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, ET.ParseError) as error:
        parser.exit(1, f"Offline runtime layout failed: {error}\n")


if __name__ == "__main__":
    main()
