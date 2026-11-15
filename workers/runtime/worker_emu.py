"""Linux file-admission adapter for the reusable Unicorn analyzer."""
from worker_common import confined
from wukong_analysis.emu import run

class EmuService:
    def __init__(self, root, trusted, isolated): self.root = root
    def run(self, params): return run(confined(self.root, params["target"], True), params)
