"""Read MSI databases and lay out verified CABs without running installer actions."""
import ctypes
from ctypes import wintypes
from pathlib import Path
import os
import re
import shutil

from scripts.worker.paths import member, no_links, regular, unique_paths

MAX_FILES = 20_000
MAX_BYTES = 2 * 1024**3
QUERIES = {
    "Directory": ("SELECT `Directory`,`Directory_Parent`,`DefaultDir` FROM `Directory`", 3),
    "Component": ("SELECT `Component`,`Directory_` FROM `Component`", 2),
    "File": ("SELECT `File`,`Component_`,`FileName`,`FileSize`,`Sequence` FROM `File`", 5),
    "Media": ("SELECT `LastSequence`,`Cabinet` FROM `Media` ORDER BY `DiskId`", 2),
}


def _windows_dll(name):
    if os.name != "nt":
        raise ValueError("MSI/CAB layout requires a Windows controller")
    return ctypes.WinDLL(name, use_last_error=True, winmode=0x800)


def read_tables(path):
    """MSIDBOPEN_READONLY; no product registration, scripts or custom actions."""
    path = regular(path)
    msi = _windows_dll("msi.dll")
    handle = wintypes.UINT
    msi.MsiOpenDatabaseW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.POINTER(handle)]
    msi.MsiDatabaseOpenViewW.argtypes = [handle, wintypes.LPCWSTR, ctypes.POINTER(handle)]
    msi.MsiViewExecute.argtypes = [handle, handle]
    msi.MsiViewFetch.argtypes = [handle, ctypes.POINTER(handle)]
    msi.MsiRecordGetStringW.argtypes = [handle, wintypes.UINT, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    msi.MsiCloseHandle.argtypes = [handle]
    database = handle()

    def checked(result):
        if result:
            raise ValueError(f"Read-only MSI query failed: {result}")

    checked(msi.MsiOpenDatabaseW(str(path), None, ctypes.byref(database)))
    tables = {}
    try:
        for table, (query, columns) in QUERIES.items():
            view = handle()
            checked(msi.MsiDatabaseOpenViewW(database, query, ctypes.byref(view)))
            try:
                checked(msi.MsiViewExecute(view, 0))
                rows = []
                while True:
                    record = handle()
                    result = msi.MsiViewFetch(view, ctypes.byref(record))
                    if result == 259:  # ERROR_NO_MORE_ITEMS
                        break
                    checked(result)
                    try:
                        values = []
                        for column in range(1, columns + 1):
                            buffer = ctypes.create_unicode_buffer(1025)
                            size = wintypes.DWORD(len(buffer))
                            checked(msi.MsiRecordGetStringW(record, column, buffer, ctypes.byref(size)))
                            values.append(buffer.value)
                        rows.append(values)
                        if len(rows) > MAX_FILES:
                            raise ValueError("MSI table exceeds row budget")
                    finally:
                        msi.MsiCloseHandle(record)
                tables[table] = rows
            finally:
                msi.MsiCloseHandle(view)
    finally:
        msi.MsiCloseHandle(database)
    return tables


def _long_name(value):
    # Target name precedes the optional source name; a long name follows '|'.
    return value.split(":", 1)[0].split("|")[-1]


def plan(tables):
    def index(rows):
        result = {row[0]: row[1:] for row in rows}
        if len(result) != len(rows):
            raise ValueError("Duplicate MSI row identity")
        return result

    directories = index(tables["Directory"])
    components = index(tables["Component"])
    resolved, active = {}, set()

    def directory(identity):
        if identity in active or identity not in directories:
            raise ValueError("Cyclic or missing MSI directory")
        if identity not in resolved:
            active.add(identity)
            parent, raw = directories[identity]
            name = _long_name(raw)
            if identity == "TARGETDIR" and not parent:
                result = ""
            else:
                if not parent:
                    raise ValueError("Unexpected MSI directory root")
                root = directory(parent)
                result = root if name == "." else "/".join(filter(None, [root, member(name)]))
            resolved[identity] = result
            active.remove(identity)
        return resolved[identity]

    media, previous = [], 0
    for last, cabinet in tables["Media"]:
        last = int(last)
        if last <= previous or last > MAX_FILES:
            raise ValueError("Invalid MSI media sequence")
        if cabinet:
            # The pinned SDK uses external CABs. Embedded/multi-volume media
            # must be implemented explicitly instead of locating ambient files.
            member(cabinet)
            if "/" in cabinet or not cabinet.lower().endswith(".cab"):
                raise ValueError("Unsupported MSI cabinet")
        media.append((previous, last, cabinet))
        previous = last
    files, ids, sequences = [], set(), set()
    for identity, component, raw, size, sequence in tables["File"]:
        size, sequence = int(size), int(sequence)
        if (not re.fullmatch(r"[A-Za-z0-9_.-]{1,200}", identity) or identity in ids
                or sequence in sequences or not 0 <= size <= MAX_BYTES or sequence <= 0):
            raise ValueError("Invalid MSI file identity, size or sequence")
        ids.add(identity)
        sequences.add(sequence)
        if component not in components:
            raise ValueError("Missing MSI file component")
        name = member(_long_name(raw))
        if "/" in name:
            raise ValueError("Invalid MSI filename")
        target = member("/".join(filter(None, [directory(components[component][0]), name])))
        matches = [cab for first, last, cab in media if first < sequence <= last]
        if len(matches) != 1:
            raise ValueError("MSI file has no unique cabinet")
        files.append(dict(id=identity, path=target, size=size, cabinet=matches[0]))
    if not files or len(files) > MAX_FILES or sum(item["size"] for item in files) > MAX_BYTES:
        raise ValueError("MSI layout exceeds file/size budget")
    unique_paths(item["path"] for item in files)
    return files


def extract_cabinet(cabinet, output, expected):
    """Only callback-approved file IDs can be decompressed into a fresh directory."""
    cabinet, output = regular(cabinet), no_links(output)
    output.mkdir()  # never overwrite/reuse an existing extraction directory
    unique_paths(expected)
    if any("/" in name for name in expected):
        raise ValueError("Cabinet member must be a single filename")
    if len(expected) > MAX_FILES or sum(expected.values()) > MAX_BYTES:
        raise ValueError("Cabinet extraction exceeds budget")
    setup = _windows_dll("setupapi.dll")

    class FileInCabinet(ctypes.Structure):
        _fields_ = [("name", wintypes.LPCWSTR), ("size", wintypes.DWORD),
                    ("error", wintypes.DWORD), ("date", wintypes.WORD),
                    ("time", wintypes.WORD), ("attributes", wintypes.WORD),
                    ("target", wintypes.WCHAR * 260)]

    class FilePaths(ctypes.Structure):
        _fields_ = [("target", wintypes.LPCWSTR), ("source", wintypes.LPCWSTR),
                    ("error", wintypes.UINT), ("flags", wintypes.DWORD)]

    callback_type = ctypes.WINFUNCTYPE(wintypes.UINT, ctypes.c_void_p, wintypes.UINT,
                                      ctypes.c_size_t, ctypes.c_size_t)
    seen, errors = set(), []

    @callback_type
    def callback(_context, notification, parameter1, _parameter2):
        try:
            if notification == 0x11:  # SPFILENOTIFY_FILEINCABINET
                info = ctypes.cast(parameter1, ctypes.POINTER(FileInCabinet)).contents
                name = info.name
                if name not in expected or name in seen or info.size != expected[name] or info.error:
                    raise ValueError("Cabinet member differs from verified MSI inventory")
                destination = no_links(output / name)
                if len(str(destination)) >= 260 or destination.exists():
                    raise ValueError("Cabinet target is ambiguous, too long or already exists")
                info.target = str(destination)
                seen.add(name)
                return 1  # FILEOP_DOIT
            if notification == 0x12:  # SPFILENOTIFY_NEEDNEWCABINET
                raise ValueError("Multi-volume cabinets are not supported")
            if notification == 0x13:
                info = ctypes.cast(parameter1, ctypes.POINTER(FilePaths)).contents
                if info.error:
                    raise ValueError(f"Cabinet extraction failed: {info.error}")
            return 0
        except Exception as error:  # never unwind through a C callback
            errors.append(str(error))
            return 0

    setup.SetupIterateCabinetW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, callback_type, ctypes.c_void_p]
    setup.SetupIterateCabinetW.restype = wintypes.BOOL
    success = setup.SetupIterateCabinetW(str(cabinet), 0, callback, None)
    if not success or errors or seen != set(expected):
        raise ValueError(errors[0] if errors else f"Incomplete cabinet extraction: {ctypes.get_last_error()}")
    actual = {path.name for path in output.iterdir()}
    if actual != set(expected):
        raise ValueError("Cabinet produced unexpected files")
    for name, size in expected.items():
        if regular(output / name).stat().st_size != size:
            raise ValueError("Extracted cabinet member size mismatch")


def layout(database, source, output, scratch):
    files = plan(read_tables(database))
    output, source, scratch = no_links(output), no_links(source), no_links(scratch)
    output.mkdir()
    scratch.mkdir()
    cabinets = sorted({item["cabinet"] for item in files if item["cabinet"]})
    for cabinet in cabinets:
        selected = [item for item in files if item["cabinet"] == cabinet]
        directory = scratch / cabinet
        extract_cabinet(source / cabinet, directory, {item["id"]: item["size"] for item in selected})
        for item in selected:
            target = no_links(output / item["path"])
            target.parent.mkdir(parents=True, exist_ok=True)
            # Same private staging filesystem; exclusive creation, not overwrite.
            os.link(directory / item["id"], target)
    for item in files:
        if not item["cabinet"]:
            original = regular(source / Path(item["path"]).name)
            if original.stat().st_size != item["size"]:
                raise ValueError("Uncompressed MSI payload size mismatch")
            target = no_links(output / item["path"])
            target.parent.mkdir(parents=True, exist_ok=True)
            with original.open("rb") as incoming, target.open("xb") as outgoing:
                shutil.copyfileobj(incoming, outgoing, 1024 * 1024)
    return files
