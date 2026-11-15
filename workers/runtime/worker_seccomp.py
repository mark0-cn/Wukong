"""Linux input confinement adapter for shared classic-BPF analysis."""

from worker_common import confined
from pathlib import Path
from typing import Any
from wukong_analysis.seccomp import analyze


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
