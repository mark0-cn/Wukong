"""Validate a sealed local executable without restoring token privileges."""
import os
from pathlib import Path


def local_path(value):
    path = Path(value)
    if os.name != "nt":
        return path.resolve(strict=True)
    # Windows normalized-name queries can require privileges deliberately absent
    # from the metadata worker; validate the existing local components instead.
    drive = path.drive.removeprefix("\\\\?\\")
    if not path.is_absolute() or len(drive) != 2 or not drive[0].isascii() or not drive[0].isalpha() or drive[1] != ":":
        raise ValueError("native tools require an absolute local drive path")
    for part in path.parts[1:]:
        if part in {".", ".."} or part.endswith((".", " ")) or any(c in part for c in (":", ";", "\0")):
            raise ValueError("native tool path contains an ambiguous component")
    for current in [path, *path.parents]:
        metadata = current.lstat()
        if current.is_symlink() or getattr(metadata, "st_file_attributes", 0) & 0x400:
            raise ValueError("native tool paths cannot contain links or reparse points")
    return path
