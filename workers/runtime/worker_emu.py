"""Linux file-admission adapter for the reusable Unicorn analyzer."""

from worker_common import confined
from pathlib import Path
from typing import Any
from wukong_analysis.emu import run


class EmuService:
    def __init__(self, root: Path) -> None:
        self.root = root

    def run(self, params: dict[str, Any]) -> dict[str, Any]:
        return run(confined(self.root, params["target"], True), params)
