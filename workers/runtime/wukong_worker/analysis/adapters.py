"""Confined input adapters for independent, stateless analysis implementations."""

from pathlib import Path
from typing import Any
from wukong_worker.artifacts.common import confined
from wukong_analysis.emu import run
from wukong_analysis.libc import identify
from wukong_analysis.seccomp import analyze


class EmuService:
    def __init__(self, root: Path) -> None:
        self.root = root

    def run(self, params: dict[str, Any]) -> dict[str, Any]:
        return run(confined(self.root, params["target"], True), params)


class LibcService:
    def __init__(self, root: Path) -> None:
        self.root = root

    def identify(self, params: dict[str, Any]) -> dict[str, Any]:
        paths = params.get("candidates")
        supplied = paths is not None
        if supplied:
            if not 1 <= len(paths) <= 16:
                raise ValueError("supply 1..16 in-run libc candidates")
            paths = [confined(self.root, path, True) for path in paths]
        else:
            paths = [
                path
                for path in [
                    Path("/lib/x86_64-linux-gnu/libc.so.6"),
                    Path("/lib/i386-linux-gnu/libc.so.6"),
                ]
                if path.is_file()
            ]
        target = confined(self.root, params["target"], True) if params.get("target") else None
        return identify(
            params, paths, target, "supplied_corpus" if supplied else "worker_libc_only"
        )


class SeccompService:
    def __init__(self, root: Path) -> None:
        self.root = root

    def analyze(self, params: dict[str, Any]) -> dict[str, Any]:
        path = confined(self.root, params["filter"], True)
        if path.stat().st_size > 32768:
            raise ValueError("filter exceeds 32 KiB")
        return analyze(
            path.read_bytes(), params.get("endianness") or "little", params.get("probes")
        )
