"""Portable package member names and no-link controller staging paths."""
import os
from pathlib import Path
import re
import stat


def member(value):
    if not isinstance(value, str) or len(value) > 220 or "\\" in value:
        raise ValueError("Invalid offline member path")
    parts = value.split("/")
    for part in parts:
        if (not re.fullmatch(r"[A-Za-z0-9_. -]+", part) or part in {".", ".."}
                or part.startswith(" ") or part.endswith((".", " "))
                or part.split(".")[0].endswith(" ")
                or re.fullmatch(r"(?i)(con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\..*)?", part)):
            raise ValueError("Unsafe offline member path: " + value)
    return value


def no_links(path):
    """Reject links/reparse points before resolving; missing leaves are permitted."""
    if ".." in Path(path).parts:
        raise ValueError("Relative aliases are forbidden in offline staging roots")
    path = Path(os.path.abspath(path))
    for current in reversed((path, *path.parents)):
        try:
            metadata = current.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(metadata.st_mode) or getattr(metadata, "st_file_attributes", 0) & 0x400:
            raise ValueError("Linked offline staging path is forbidden")
        if current != path and not stat.S_ISDIR(metadata.st_mode):
            raise ValueError("Offline staging ancestor is not a directory")
    return path


def regular(path):
    path = no_links(path)
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError("Expected a regular offline payload")
    return path


def unique_paths(names):
    folded = set()
    for name in names:
        member(name)
        if name.lower() in folded:
            raise ValueError("Case-colliding offline members")
        folded.add(name.lower())
    for name in folded:
        parent = name
        while "/" in parent:
            parent = parent.rsplit("/", 1)[0]
            if parent in folded:
                raise ValueError("Offline file/directory collision")
