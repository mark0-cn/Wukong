"""One bounded analysis invocation. No worker identity, leases, daemon or job ledger."""
import json
from pathlib import Path
import sys


def main():
    configuration = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    method, params = configuration["method"], configuration["params"]
    from worker_seccomp import SeccompService
    if method not in SeccompService.methods:
        raise ValueError("analysis method is not supported by this offline package")
    service = SeccompService(Path.cwd(), False, False)
    result = service.dispatch(method, params)
    print(json.dumps({"result": result}, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"error": str(error)[:2048]}), flush=True)
        raise SystemExit(1)
