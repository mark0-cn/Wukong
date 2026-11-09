"""Offline libc identification against the worker's own libc.

`libc.identify` maps a leaked runtime address to the libc base of the
worker's local libc by matching the leaked value against a named symbol's
offset. This is fully offline: the worker identifies only the libc it
itself runs, which is the case that matters when the target executes in
this worker. A future extension can bundle a libc-database subset for
remote targets whose libc differs from the worker's.
"""
from pathlib import Path

from worker_common import parse_address


class LibcService:
    methods = frozenset(["libc.identify"])

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated

    def dispatch(self, method, params):
        if method == "libc.identify":
            return self.identify(params)
        raise ValueError("unsupported LibcService method: " + method)

    def identify(self, params):
        leaked = parse_address(params["leaked"])
        symbol = params.get("symbol") or "puts"
        try:
            from pwn import ELF  # noqa: PLC0415 - heavy import
        except ImportError as error:
            raise ValueError("pwntools is not installed in this worker") from error
        candidates = []
        for path in [Path("/lib/x86_64-linux-gnu/libc.so.6"),
                     Path("/lib/i386-linux-gnu/libc.so.6")]:
            if not path.is_file():
                continue
            elf = ELF(str(path), checksec=False)
            offset = elf.symbols.get(symbol)
            if not offset:
                continue
            base = leaked - offset
            if base > 0 and base & 0xFFF == 0:
                candidates.append({
                    "path": str(path),
                    "symbol": symbol,
                    "symbol_offset": offset,
                    "base": base,
                    "system_offset": elf.symbols.get("system"),
                    "binsh_offset": next(elf.search(b"/bin/sh\x00"), None),
                })
        if not candidates:
            return {"matched": False, "leaked": leaked, "symbol": symbol,
                    "note": "no local libc matches; offline identification covers the worker's OWN libc only"}
        return {"matched": True, "leaked": leaked, "symbol": symbol, "candidates": candidates[:4]}
