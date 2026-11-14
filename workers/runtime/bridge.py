#!/usr/bin/env python3
"""Worker command-line entry point."""
import sys
# Signed deployments are immutable. Set this before importing worker modules,
# including when a daemon/job supervisor starts a new embedded interpreter.
sys.dont_write_bytecode = True
from pathlib import Path
import argparse
import json
import os
from worker_common import MAX_RPC, job_dir
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
            if len(raw) > MAX_RPC:
                raise ValueError("request exceeds size limit")
            response = relay_call(endpoint, raw)
            sys.stdout.buffer.write(response)
            sys.stdout.buffer.flush()
    else:
        parser.error("select --stdio, --daemon or --run-job")

if __name__ == "__main__":
    main()
