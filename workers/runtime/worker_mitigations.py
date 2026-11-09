"""Protection declarations and observations; never changes target protection state."""
from pathlib import Path
import platform
import struct


def gnu_property_features(data, little_endian, bits):
    """Decode bounded PT_NOTE/PT_GNU_PROPERTY bytes; values describe ELF only."""
    endian = "<" if little_endian else ">"
    offset, values = 0, []
    while offset + 12 <= len(data):
        namesz, descsz, kind = struct.unpack_from(endian+"III",data,offset)
        offset += 12
        name_end = offset+namesz
        desc_start = (name_end+3)&~3
        desc_end = desc_start+descsz
        if desc_end > len(data): raise ValueError("truncated ELF note")
        if kind == 5 and data[offset:name_end].rstrip(b"\0") == b"GNU":
            cursor = desc_start
            align = 8 if bits == 64 else 4
            while cursor < desc_end:
                if cursor+8 > desc_end: raise ValueError("truncated GNU property")
                prop, size = struct.unpack_from(endian+"II",data,cursor)
                cursor += 8
                if cursor+size > desc_end: raise ValueError("truncated GNU property value")
                if prop == 0xc0000002:
                    if size != 4: raise ValueError("invalid x86 feature property size")
                    values.append(struct.unpack_from(endian+"I",data,cursor)[0])
                cursor = desc_start + ((cursor+size-desc_start+align-1)//align)*align
        offset = (desc_end+3)&~3
    if any(data[offset:]): raise ValueError("trailing truncated ELF note")
    return values


def declared_cet(elf):
    if elf["e_machine"] not in {"EM_386","EM_X86_64"}:
        return {"state":"not_applicable", "source":"non-x86 ELF"}
    values=[]
    for segment in elf.iter_segments():
        if segment["p_type"] in {"PT_NOTE","PT_GNU_PROPERTY"}:
            values.extend(gnu_property_features(segment.data(),elf.little_endian,elf.elfclass))
    values=sorted(set(values))
    if len(values)>1: return {"state":"conflicting_declarations", "values":values}
    value=values[0] if values else 0
    return {"state":"declared" if values else "not_declared", "ibt":bool(value&1), "shstk":bool(value&2),
            "property_values":values, "source":"GNU_PROPERTY_X86_FEATURE_1_AND", "runtime_enabled":"unknown"}


def parse_status(text):
    fields=dict(line.split(":",1) for line in text.splitlines() if ":" in line)
    fields={k:v.strip() for k,v in fields.items()}
    features=fields.get("x86_Thread_features")
    return {"seccomp_mode":int(fields["Seccomp"]) if fields.get("Seccomp") in {"0","1","2"} else None,
            "seccomp_filters":fields.get("Seccomp_filters"), "seccomp_policy":"unknown",
            "no_new_privs":fields.get("NoNewPrivs"),
            "shstk_enabled":("shstk" in features.split()) if features is not None else None,
            "thread_features":features, "thread_features_locked":fields.get("x86_Thread_features_locked"),
            "ibt_enabled":None}


def read_bounded(path, limit=65536):
    with Path(path).open(encoding="utf-8",errors="replace") as stream: text=stream.read(limit+1)
    if len(text)>limit: raise ValueError("observation exceeds bound")
    return text


def thread_observations(proc):
    observations=[]
    complete=True
    try:
        for path in (proc/"task").iterdir():
            if not path.name.isdigit(): continue
            if len(observations)>=64:
                complete=False
                break
            try:
                raw=read_bounded(path/"status")
                observations.append({"tid":int(path.name), "source":"/proc/PID/task/TID/status",
                                     "values":parse_status(raw), "raw_status":raw})
            except (OSError,ValueError) as error:
                observations.append({"tid":int(path.name), "state":"unknown", "error":str(error)[:256]})
    except OSError as error:
        return {"state":"unknown", "error":str(error)[:256]}
    return {"state":"observed", "threads":observations, "complete":complete,
            "note":"Per-thread observations at read time, not an atomic process-wide guarantee. Policy rules and IBT enablement remain unknown."}


def environment_support():
    result={"kernel":platform.release(), "system":platform.system(), "userspace_shstk":"unknown",
            "seccomp_filter_interface":"unknown", "userspace_ibt_enabled":"unknown"}
    try:
        cpuinfo=read_bounded("/proc/cpuinfo",1048576)
        flags=[set(line.split(":",1)[1].split()) for line in cpuinfo.splitlines() if line.split(":",1)[0].strip()=="flags"]
        if flags:
            count=sum("user_shstk" in item for item in flags)
            result["userspace_shstk"]="advertised_on_all_reported_cpus" if count==len(flags) else "mixed" if count else "not_advertised"
            result["reported_cpus"]=len(flags)
        result["shstk_source"]="/proc/cpuinfo user_shstk; absence does not identify the missing hardware/kernel/config prerequisite"
    except (OSError,ValueError) as error: result["shstk_error"]=str(error)[:256]
    try:
        result["seccomp_actions_available"]=read_bounded("/proc/sys/kernel/seccomp/actions_avail",4096).split()
        result["seccomp_filter_interface"]="observed"
    except (OSError,ValueError) as error: result["seccomp_error"]=str(error)[:256]
    return result
