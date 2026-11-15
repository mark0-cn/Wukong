"""Packaged worker implementation; deploy with bridge.py and the protocol schema."""

from pathlib import Path

PACKAGE_ROOT = Path(__file__).resolve().parent
RUNTIME_ROOT = PACKAGE_ROOT.parent
BRIDGE_PATH = RUNTIME_ROOT / "bridge.py"
