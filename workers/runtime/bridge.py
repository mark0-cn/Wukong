#!/usr/bin/env python3
"""Worker command-line entry point."""
from pathlib import Path
import argparse
import json
import os
import sys
from worker_common import MAX_RPC, job_dir, rpc_error
from worker_job_supervisor import supervise_job
from worker_transport import ensure_daemon, relay_call, serve

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--stdio", action="store_true")
    parser.add_argument("--daemon", action="store_true")
    parser.add_argument("--run-job")
    parser.add_argument("--trusted-local", action="store_true")
    args = parser.parse_args()
    root = Path(args.state).resolve()
    root.mkdir(parents=True, exist_ok=True)
    root.chmod(0o700)
    if args.trusted_local and hasattr(os, "geteuid") and os.geteuid() == 0:
        raise SystemExit("--trusted-local is refused for a root daemon: jobs would "
                         "escape the credential boundary; run the daemon unprivileged "
                         "or drop the flag and use isolated execution")
    if args.run_job:
        supervise_job(root, job_dir(root, args.run_job))
    elif args.daemon:
        serve(root, args.trusted_local)
    elif args.stdio:
        endpoint = ensure_daemon(root, args.trusted_local)
        while True:
            raw = sys.stdin.buffer.readline(MAX_RPC + 1)
            if not raw:
                break
            request = {}
            try:
                if len(raw) > MAX_RPC:
                    raise ValueError("request exceeds size limit")
                request = json.loads(raw)
                response = relay_call(endpoint, request)
            except Exception as error:
                response = {"jsonrpc": "2.0", "id": request.get("id"), "error": rpc_error(error)}
            # Explicit UTF-8 bytes, ASCII-escaped JSON: the console codec
            # (e.g. GBK on Windows) must never be able to break a frame when
            # a result carries replacement characters.
            sys.stdout.buffer.write(json.dumps(response, ensure_ascii=True).encode("utf-8") + b"\n")
            sys.stdout.buffer.flush()
    else:
        parser.error("select --stdio, --daemon or --run-job")

if __name__ == "__main__":
    main()
