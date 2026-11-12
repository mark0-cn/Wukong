"""Build known, static-CRT self-test fixtures; the offline VM needs no compiler."""
import os
from pathlib import Path
import subprocess

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts.worker.paths import no_links, regular
from scripts.worker.pe import require_static_crt


def compiler_environment(architecture):
    if architecture not in {"x64", "x86"}:
        raise ValueError("Unsupported self-test architecture")
    installation = os.environ.get("VSINSTALLDIR")
    if not installation:
        locator = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        installation = subprocess.check_output([str(regular(locator)), "-latest", "-products", "*", "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"], text=True).strip()
    setup = regular(Path(installation) / "Common7/Tools/VsDevCmd.bat")
    if any(value in str(setup) for value in ['"', '%', '\n', '\r']):
        raise ValueError("Ambiguous MSVC setup path")
    command = f'cmd.exe /u /d /v:off /s /c ""{setup}" -no_logo -host_arch=x64 -arch={architecture} >nul && set"'
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise ValueError("MSVC self-test toolchain initialization failed")
    environment = {key.upper(): value for key, value in os.environ.items()}
    for line in result.stdout.decode("utf-16-le").splitlines():
        if "=" in line and not line.startswith("="):
            key, value = line.split("=", 1)
            environment[key.upper()] = value
    return environment


def build(output, inspector=None):
    output = no_links(output)
    source = regular(ROOT / "workers/windows/fixtures/probe.c")
    artifacts = {}
    for architecture in ["x64", "x86"]:
        directory = output / "selftest" / architecture
        directory.mkdir(parents=True, exist_ok=True)
        target = directory / "wukong-probe.exe"
        pdb = target.with_suffix(".pdb")
        environment = compiler_environment(architecture)
        compiler = regular(Path(environment["VCTOOLSINSTALLDIR"]) / "bin/Hostx64" / architecture / "cl.exe")
        subprocess.run([str(compiler), "/nologo", "/W4", "/WX", "/Zi", "/Od", "/MT",
            "/Fe:" + str(target), str(source), "dbghelp.lib", "/link", "/DEBUG", "/PDB:" + str(pdb)],
            cwd=directory, env=environment, check=True)
        artifacts[architecture] = {"binary": target.relative_to(output).as_posix(), "binary_sha256": sha256(target),
            "pdb": pdb.relative_to(output).as_posix(), "pdb_sha256": sha256(pdb),
            "imported_libraries": require_static_crt(target, architecture, inspector), "source_sha256": sha256(source)}
    return {"schema": 1, "architectures": artifacts}
