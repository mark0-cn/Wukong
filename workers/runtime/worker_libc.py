"""Offline, ambiguous-by-design libc candidate comparison; never loads a DSO."""
import hashlib
import io
from pathlib import Path
from worker_common import MAX_FILE, confined, parse_address


def inspect_candidate(path, leaks, target_abi=None):
    from elftools.elf.elffile import ELFFile
    if path.stat().st_size > MAX_FILE: raise ValueError("candidate exceeds 32 MiB")
    data = path.read_bytes()
    if len(data) > MAX_FILE: raise ValueError("candidate exceeds 32 MiB")
    elf = ELFFile(io.BytesIO(data))
    abi = (elf["e_machine"], elf.elfclass, elf.little_endian)
    if elf["e_type"] != "ET_DYN": raise ValueError("candidate must be an ELF shared object")
    identity = {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(),
                "architecture": elf.get_machine_arch(), "bits": elf.elfclass,
                "endianness": "little" if elf.little_endian else "big", "build_id": None}
    for segment in elf.iter_segments():
        if segment["p_type"] == "PT_NOTE":
            for note in segment.iter_notes():
                if note["n_type"] == "NT_GNU_BUILD_ID": identity["build_id"] = str(note["n_desc"])
    if target_abi and abi != target_abi: return {**identity, "candidate_match": False, "reason": "architecture mismatch"}
    symbols = elf.get_section_by_name(".dynsym")
    if symbols is None or symbols.num_symbols() > 65536: raise ValueError("missing or oversized dynamic symbol table")
    wanted = {leak["symbol"] for leak in leaks} | {"system"}
    offsets, ifuncs = {}, set()
    for symbol in symbols.iter_symbols():
        if symbol.name not in wanted or not isinstance(symbol["st_shndx"],int) or symbol["st_info"]["type"] == "STT_TLS": continue
        if symbol["st_info"]["type"] in {"STT_GNU_IFUNC", "STT_LOOS"}: ifuncs.add(symbol.name)
        offsets.setdefault(symbol.name, set()).add(int(symbol["st_value"]))
    bases, observations = set(), []
    for leak in leaks:
        name = leak["symbol"]
        if name in ifuncs or len(offsets.get(name, [])) != 1:
            return {**identity, "candidate_match": False, "reason": "missing, ambiguous or IFUNC symbol: " + name}
        offset = next(iter(offsets[name]))
        base = leak["address"] - offset - leak.get("addend", 0)
        bases.add(base)
        observations.append({**leak, "symbol_offset": offset, "derived_base": base})
    base = next(iter(bases))
    matched = len(bases) == 1 and 0 < base < 2**elf.elfclass and base % 4096 == 0 and all(0 <= leak["address"] < 2**elf.elfclass for leak in leaks)
    result = {**identity, "candidate_match": matched, "observations": observations}
    if matched:
        result["base"] = base
        result["system_offset"] = next(iter(offsets["system"])) if len(offsets.get("system", [])) == 1 and "system" not in ifuncs else None
        result["binsh_offset"] = None
        for segment in elf.iter_segments():
            if segment["p_type"] != "PT_LOAD": continue
            start, size = int(segment["p_offset"]), int(segment["p_filesz"])
            found = data.find(b"/bin/sh\0", start, start+size)
            if found >= 0:
                result["binsh_offset"] = int(segment["p_vaddr"])+found-start
                break
    return result


class LibcService:

    def __init__(self, root, trusted, isolated):
        self.root = root

    def dispatch(self, method, params):
        if method == "libc.identify":
            return self.identify(params)
        raise ValueError("unsupported LibcService method: " + method)

    def identify(self, params):
        from elftools.elf.elffile import ELFFile
        from elftools.common.exceptions import ELFError
        leaks = [dict(leak) for leak in (params.get("leaks") or [])]
        if params.get("leaked") is not None:
            leaks.insert(0, {"address": params["leaked"], "symbol": params.get("symbol") or "puts"})
        if not 1 <= len(leaks) <= 16: raise ValueError("supply 1..16 same-process symbol leaks")
        for leak in leaks:
            leak["address"] = parse_address(leak["address"])
            if not isinstance(leak.get("symbol"), str) or not 1 <= len(leak["symbol"]) <= 256:
                raise ValueError("leak symbol must contain 1..256 characters")
            addend = leak.get("addend", 0)
            if not isinstance(addend, int) or isinstance(addend, bool) or not -(2**63) <= addend < 2**63:
                raise ValueError("leak addend must be int64")
        paths = params.get("candidates")
        supplied = paths is not None
        if supplied:
            if not 1 <= len(paths) <= 16: raise ValueError("supply 1..16 in-run libc candidates")
            paths = [confined(self.root, path, True) for path in paths]
        else:
            paths = [p for p in [Path("/lib/x86_64-linux-gnu/libc.so.6"), Path("/lib/i386-linux-gnu/libc.so.6")] if p.is_file()]
        if sum(p.stat().st_size for p in paths) > 64*1024*1024: raise ValueError("candidate corpus exceeds 64 MiB")
        target_abi = None
        if params.get("target"):
            path = confined(self.root, params["target"], True)
            if path.stat().st_size > MAX_FILE: raise ValueError("target exceeds 32 MiB")
            with path.open("rb") as stream:
                elf = ELFFile(stream)
                target_abi = (elf["e_machine"], elf.elfclass, elf.little_endian)
        compared, seen = [], set()
        for path in paths:
            try:
                item = inspect_candidate(path, leaks, target_abi)
            except (OSError, ValueError, ELFError) as error:
                item = {"path": str(path), "candidate_match": False, "error": str(error)[:256]}
            if item.get("sha256") in seen: continue
            if item.get("sha256"): seen.add(item["sha256"])
            compared.append(item)
        candidates = [item for item in compared if item["candidate_match"]]
        return {"matched": False, "candidate_match": bool(candidates), "ambiguous": len(candidates) > 1,
                "identity_status": "candidate_only" if candidates else "unresolved", "candidates": candidates,
                "compared": compared, "source": "supplied_corpus" if supplied else "worker_libc_only",
                "note": "Consistent leaks select candidates within this corpus, not a confirmed mapped libc. Confirm actual SHA256/Build ID with runtime_profile; no match does not identify a different libc."}
