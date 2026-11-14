"""Linux corpus selection and confinement; comparison lives in wukong_analysis."""
from pathlib import Path
from worker_common import confined
from wukong_analysis.libc import identify

class LibcService:
    def __init__(self, root, trusted, isolated): self.root = root
    def identify(self, params):
        paths = params.get("candidates")
        supplied = paths is not None
        if supplied:
            if not 1 <= len(paths) <= 16: raise ValueError("supply 1..16 in-run libc candidates")
            paths = [confined(self.root, path, True) for path in paths]
        else:
            paths = [path for path in [Path("/lib/x86_64-linux-gnu/libc.so.6"), Path("/lib/i386-linux-gnu/libc.so.6")] if path.is_file()]
        target = confined(self.root, params["target"], True) if params.get("target") else None
        return identify(params, paths, target, "supplied_corpus" if supplied else "worker_libc_only")
