"""Explicit unit/live test entry point for a checkout or the worker image."""
import argparse
from pathlib import Path
import runpy
import sys
import unittest

ROOT = Path(__file__).resolve().parent
RUNTIME = ROOT.parent / "runtime"
# One test bootstrap owns these paths; production runtime imports stay unchanged.
sys.path[:0] = [str(ROOT), str(RUNTIME)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=["unit", "integration", "isolation", "ghidra", "launch"])
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    remaining = args.arguments
    if args.suite == "unit":
        options = argparse.ArgumentParser()
        options.add_argument("-p", "--pattern", default="test_*.py")
        options.add_argument("-v", "--verbose", action="store_true")
        unit = options.parse_args(remaining)
        suite = unittest.defaultTestLoader.discover(str(ROOT / "unit"), pattern=unit.pattern, top_level_dir=str(ROOT))
        if suite.countTestCases() == 0:
            raise SystemExit("No worker tests matched the requested pattern")
        result = unittest.TextTestRunner(verbosity=2 if unit.verbose else 1).run(suite)
        raise SystemExit(0 if result.wasSuccessful() else 1)
    module = {"integration": "runtime", "isolation": "isolation", "ghidra": "ghidra", "launch":"launch"}[args.suite]
    sys.argv = [str(ROOT / "integration" / (module + ".py")), *remaining]
    runpy.run_module("integration." + module, run_name="__main__")


if __name__ == "__main__":
    main()
