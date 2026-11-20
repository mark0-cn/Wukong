"""Build known, static-CRT self-test fixtures; the offline VM needs no compiler."""

from pathlib import Path
import subprocess

from scripts._lib.checksums import sha256
from scripts._lib.msvc import compiler_environment
from scripts._lib.repository import ROOT
from scripts.worker.paths import no_links, regular
from scripts.worker.pe import require_static_crt


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
        compiler = regular(
            Path(environment["VCTOOLSINSTALLDIR"]) / "bin/Hostx64" / architecture / "cl.exe"
        )
        subprocess.run(
            [
                str(compiler),
                "/nologo",
                "/W4",
                "/WX",
                "/Zi",
                "/Od",
                "/MT",
                "/Fe:" + str(target),
                str(source),
                "dbghelp.lib",
                "advapi32.lib",
                "/link",
                "/DEBUG",
                "/PDB:" + str(pdb),
            ],
            cwd=directory,
            env=environment,
            check=True,
        )
        dependency_source = regular(ROOT / "workers/windows/fixtures/dependency.c")
        dependency = directory / "wukong-fixture.dll"
        export = "wukong_fixture_value"
        subprocess.run(
            [
                str(compiler),
                "/nologo",
                "/W4",
                "/WX",
                "/O2",
                "/MT",
                "/LD",
                "/Fe:" + str(dependency),
                str(dependency_source),
                "/link",
                "/EXPORT:" + export,
            ],
            cwd=directory,
            env=environment,
            check=True,
        )
        artifacts[architecture] = {
            "binary": target.relative_to(output).as_posix(),
            "binary_sha256": sha256(target),
            "pdb": pdb.relative_to(output).as_posix(),
            "pdb_sha256": sha256(pdb),
            "imported_libraries": require_static_crt(target, architecture, inspector),
            "source_sha256": sha256(source),
            "dependency": {
                "binary": dependency.relative_to(output).as_posix(),
                "sha256": sha256(dependency),
                "source_sha256": sha256(dependency_source),
                "imported_libraries": require_static_crt(dependency, architecture, inspector),
            },
        }
    return {"schema": 1, "architectures": artifacts}
