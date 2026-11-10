"""Locate the checkout without depending on a script's nesting depth or cwd."""
from pathlib import Path


def find_root(start):
    path = Path(start).resolve()
    for candidate in (path, *path.parents):
        if (candidate / 'Cargo.toml').is_file() and (candidate / 'rust-toolchain.toml').is_file():
            return candidate
    raise RuntimeError('Cannot locate re-agent repository root')


ROOT = find_root(__file__)
