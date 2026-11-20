"""Discover and validate the real Windows MSVC environment for native builds."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from scripts._lib.repository import ROOT


NATIVE_PACKAGES = ("aws-lc-sys", "ring", "libsqlite3-sys")
EXECUTABLES = ("cargo", "cl.exe", "link.exe", "lib.exe")
DIAGNOSTIC_VARIABLES = (
    "VSINSTALLDIR",
    "VCTOOLSINSTALLDIR",
    "VCTOOLSVERSION",
    "WINDOWSSDKDIR",
    "WINDOWSSDKVERSION",
    "CC",
    "CXX",
    "TMP",
    "TEMP",
)


def compiler_environment(architecture="x64"):
    """Return a child environment initialized by the selected VsDevCmd."""

    if architecture not in {"x64", "x86"}:
        raise ValueError("Unsupported MSVC architecture")
    installation = os.environ.get("VSINSTALLDIR")
    if not installation:
        locator = (
            Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
            / "Microsoft Visual Studio/Installer/vswhere.exe"
        )
        if not locator.is_file():
            raise ValueError(f"Visual Studio locator is missing: {locator}")
        installation = subprocess.check_output(
            [
                str(locator),
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ],
            text=True,
        ).strip()
    setup = Path(installation) / "Common7/Tools/VsDevCmd.bat"
    if not setup.is_file():
        raise ValueError(f"Visual Studio developer command setup is missing: {setup}")
    if any(value in str(setup) for value in ['"', "%", "\n", "\r"]):
        raise ValueError("Ambiguous MSVC setup path")
    command = f'cmd.exe /u /d /v:off /s /c ""{setup}" -no_logo -host_arch=x64 -arch={architecture} >nul && set"'
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise ValueError("MSVC toolchain initialization failed")
    environment = {key.upper(): value for key, value in os.environ.items()}
    for line in result.stdout.decode("utf-16-le").splitlines():
        if "=" in line and not line.startswith("="):
            key, value = line.split("=", 1)
            environment[key.upper()] = value
    return environment


def workspace_temporary(root=ROOT):
    """Create the task-owned temporary directory used by compiler children."""

    root = root if isinstance(root, Path) else Path(root)
    temporary = root / "target/tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    return temporary.resolve()


def build_environment(root=ROOT):
    """Initialize MSVC and route compiler/linker temporary writes into the workspace."""

    environment = compiler_environment("x64")
    temporary = workspace_temporary(root)
    environment["TMP"] = str(temporary)
    environment["TEMP"] = str(temporary)
    return environment


def executable(name, environment):
    """Resolve one required tool without dumping or modifying the environment."""

    path = shutil.which(name, path=environment.get("PATH", ""))
    if path is None:
        raise ValueError(f"The initialized MSVC environment does not provide {name}")
    # Do not resolve symlinks here: rustup's cargo.exe proxy may resolve to
    # rustup.exe, and invoking that target changes argument interpretation.
    return str(Path(path).absolute())


def diagnostics(environment):
    """Return bounded, non-secret toolchain diagnostics suitable for build logs."""

    result = {key: environment[key] for key in DIAGNOSTIC_VARIABLES if environment.get(key)}
    for name in EXECUTABLES:
        path = shutil.which(name, path=environment.get("PATH", ""))
        if path:
            result[name] = str(Path(path).absolute())
    return result


def _probe_command(command, environment, directory, label):
    result = subprocess.run(
        [str(value) for value in command],
        cwd=directory,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
    )
    if result.returncode:
        output = (result.stdout + result.stderr)[-4000:].strip()
        raise ValueError(
            f"MSVC {label} probe failed with exit code {result.returncode}"
            + (f":\n{output}" if output else "")
        )


def preflight(environment, root=ROOT):
    """Compile, archive and link a small SDK program through real child processes."""

    tools = {name: executable(name, environment) for name in EXECUTABLES[1:]}
    parent = Path(root) / "target/msvc-preflight"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="toolchain-", dir=parent) as directory:
        directory = Path(directory)
        source = directory / "probe.c"
        object_file = directory / "probe.obj"
        library = directory / "probe.lib"
        binary = directory / "probe.exe"
        source.write_text(
            "#include <stdint.h>\nint main(void) { uint64_t value = 0; return (int)value; }\n",
            encoding="utf-8",
        )
        _probe_command(
            [
                tools["cl.exe"],
                "/nologo",
                "/W4",
                "/WX",
                "/c",
                source,
                f"/Fo{object_file}",
            ],
            environment,
            directory,
            "compiler/SDK",
        )
        _probe_command(
            [tools["lib.exe"], "/NOLOGO", f"/OUT:{library}", object_file],
            environment,
            directory,
            "archiver/temp",
        )
        _probe_command(
            [tools["link.exe"], "/NOLOGO", f"/OUT:{binary}", object_file],
            environment,
            directory,
            "linker/temp",
        )
    return tools


def native_preflight(environment, root=ROOT, target=None):
    """Cold-build native dependencies through Cargo without using a restored cache."""

    cargo = executable("cargo", environment)
    parent = Path(root) / "target/msvc-preflight"
    parent.mkdir(parents=True, exist_ok=True)
    owned = target is None
    target = Path(tempfile.mkdtemp(prefix="native-", dir=parent)) if owned else Path(target)
    if not owned and target.exists():
        if not target.is_dir():
            raise ValueError(f"Native preflight target is not a directory: {target}")
        if any(target.iterdir()):
            raise ValueError(f"Native preflight target must be new or empty: {target}")
    target.mkdir(parents=True, exist_ok=True)
    command = [cargo, "build", "--locked", "-vv"]
    for package in NATIVE_PACKAGES:
        command += ["-p", package]
    command += ["--target-dir", str(target)]
    result = subprocess.run(command, cwd=Path(root), env=environment)
    if result.returncode:
        raise ValueError(
            "Cold native dependency preflight failed; inspect the verbose Cargo output "
            f"and retained build-script files under {target}"
        )
    if owned:
        shutil.rmtree(target)
    return target
