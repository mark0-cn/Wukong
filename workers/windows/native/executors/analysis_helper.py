"""One bounded analysis invocation. No worker identity, leases, daemon or job ledger."""
import json
from pathlib import Path
import sys


def main():
    configuration = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    method, params = configuration["method"], configuration["params"]
    from wukong_analysis.seccomp import analyze
    if method != "seccomp.analyze":
        raise ValueError("analysis method is not supported by this offline package")
    path = Path(sys.argv[2])
    if path.stat().st_size > 32768:
        raise ValueError("filter exceeds 32 KiB")
    result = analyze(path.read_bytes(), params.get("endianness") or "little", params.get("probes"))
    print(json.dumps({"result": result}, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"error": str(error)[:2048]}), flush=True)
        raise SystemExit(1)
