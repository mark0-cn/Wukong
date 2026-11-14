"""Preserve native dependency, Rust library and project license documents."""
import json
from pathlib import Path
import shutil
import subprocess

from scripts._lib.repository import ROOT
from scripts.worker.paths import member, no_links, regular


def collect(output, manifest=None, supplemental=None):
    output = no_links(output)
    output.mkdir()
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--offline", "--locked", "--format-version", "1",
        "--filter-platform", "x86_64-pc-windows-msvc", "--manifest-path", str(manifest or ROOT / "workers/windows/Cargo.toml")], cwd=ROOT))
    records = []
    for package in metadata["packages"]:
        root = Path(package["manifest_path"]).parent
        candidates = [path for path in root.iterdir() if path.is_file()
                      and path.name.lower().startswith(("license", "copying", "notice", "unlicense", "copyright"))]
        if package.get("license_file"):
            candidates.append(root / package["license_file"])
        if not candidates and supplemental:
            supplement = supplemental.get((package["name"], package["version"]))
            if supplement:
                candidates.append(supplement)
        if package["source"] is None and root in {ROOT / "workers/windows", ROOT / "workers/package", ROOT / "workers/protocol"}:
            candidates.append(ROOT / "LICENSE")
        if not package.get("license") or not candidates:
            raise ValueError("Missing native dependency license documents: " + package["name"])
        directory = output / member(package["name"] + "-" + package["version"])
        directory.mkdir()
        names = []
        for path in sorted(set(candidates)):
            name = member(path.name)
            target = directory / name
            shutil.copyfile(regular(path), target)
            names.append(target.relative_to(output).as_posix())
        records.append(dict(name=package["name"], version=package["version"], license=package["license"], files=names))
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    copyright_ = regular(sysroot / "share/doc/rust/COPYRIGHT-library.html")
    shutil.copyfile(copyright_, output / "Rust-library-COPYRIGHT.html")
    (output / "native-notices.json").write_text(json.dumps(dict(schema=1, packages=records,
        rustc=subprocess.check_output(["rustc", "--version"], text=True).strip(),
        rust_library_notice="Rust-library-COPYRIGHT.html"), indent=2) + "\n", encoding="utf-8")
    return [path for path in sorted(output.rglob("*")) if path.is_file()]
