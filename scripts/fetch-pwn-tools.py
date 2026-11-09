"""Compatibility entry point; downloads the unified locked worker toolchain."""
import runpy
from pathlib import Path
runpy.run_path(str(Path(__file__).with_name("fetch-worker-deps.py")), run_name="__main__")
