"""Crash interpretation from explicit observations and files, without live sessions."""
import base64
import hashlib
import json
import re
MAX_FILE = 32 * 1024 * 1024

def integer(value):
    match = re.match(r"^(0x[0-9a-fA-F]+|[0-9]+)(?:\s|$)", str(value))
    return int(match.group(1), 16 if match.group(1).startswith("0x") else 10) if match else None


def normalize_address(address, profile):
    for mapping in profile.get("maps", []):
        if address is not None and mapping["start"] <= address < mapping["end"]:
            module = next((m for m in profile.get("modules", []) if m.get("inode") == mapping["inode"] and m.get("device") == mapping["device"]), {})
            return {"space":"mapped_file_offset", "value":address-mapping["start"]+mapping["offset"],
                    "module_sha256":module.get("sha256"), "path":mapping.get("path")}
    return {"space":"runtime", "value":address, "module_sha256":None}


def location_signature(snapshot):
    pc = snapshot.get("pc_location", {})
    if not pc.get("module_sha256") or not snapshot.get("signal"): return None
    frames = [f["location"] for f in snapshot.get("frames", []) if f.get("location", {}).get("module_sha256")][:6]
    stable = {"signal":snapshot["signal"], "pc":[pc["module_sha256"],pc["value"]],
              "frames":[[f["module_sha256"],f["value"]] for f in frames]}
    return "location:" + hashlib.sha256(json.dumps(stable,sort_keys=True).encode()).hexdigest()


def cyclic_offset(pattern: bytes, observed: bytes, n: int, alphabet_text: str):
    if len(pattern) > 1048576: raise ValueError("pattern exceeds 1 MiB")
    n=int(n); alphabet=alphabet_text.encode("ascii")
    if not 1 <= n <= 16 or len(observed)!=n or not 2 <= len(alphabet)<=128 or len(set(alphabet))!=len(alphabet):
        raise ValueError("supply exactly n observed bytes and a unique ASCII alphabet")
    from pwnlib.util.cyclic import cyclic
    if bytes(cyclic(len(pattern),alphabet=alphabet,n=n)) != pattern:
        raise ValueError("pattern does not match declared alphabet/n")
    positions=[]; start=0
    while len(positions)<2:
        found=pattern.find(observed,start)
        if found<0: break
        positions.append(found); start=found+1
    return {"pattern_sha256":hashlib.sha256(pattern).hexdigest(),"n":n,"alphabet":alphabet_text,
        "observed_base64":base64.b64encode(observed).decode("ascii"),"offset":positions[0] if len(positions)==1 else None,
        "status":"unique" if len(positions)==1 else "ambiguous" if positions else "not_found"}


def inspect_core(path):
    if path.stat().st_size > MAX_FILE: raise ValueError("core exceeds 32 MiB; use live debugger snapshot")
    from pwn import Corefile
    core=Corefile(str(path))
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    mappings=[{"start":m.start,"end":m.stop,"path":m.name,"flags":m.flags} for m in core.mappings][:512]
    result={"source":"core","core_sha256":digest,
        "pc":core.pc,"sp":core.sp,"fault_address":core.fault_addr,"signal":core.signal,
        "registers":core.registers,"mappings":mappings,"signature_strength":"unresolved",
        "note":"Core mapping names do not establish original module hashes. No location signature is fabricated."}
    try: result["stack_base64"]=base64.b64encode(core.read(core.sp,128)).decode()
    except Exception as error: result["stack_error"]=str(error)[:256]
    return result
