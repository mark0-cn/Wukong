"""Run Cargo (or an explicit native build command) with validated real MSVC tools."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from scripts._lib import msvc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Validate MSVC, SDK, archiver, linker and child-process temp writes only",
    )
    parser.add_argument(
        "--native-preflight",
        action="store_true",
        help="Cold-build aws-lc-sys, ring and libsqlite3-sys without a restored Cargo cache",
    )
    parser.add_argument(
        "--preflight-target",
        type=Path,
        help="New/empty native preflight target retained after a failure (default: task-owned temp)",
    )
    parser.add_argument(
        "--exec",
        action="store_true",
        help="Run the remaining command instead of implicitly prefixing it with Cargo",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = list(args.command)
    if command and command[0] == "--":
        command.pop(0)
    if os.name != "nt":
        parser.exit(1, "The MSVC build entry point is only available on Windows.\n")
    if args.preflight_target and not args.native_preflight:
        parser.error("--preflight-target requires --native-preflight")
    if args.check and (command or args.native_preflight or args.exec):
        parser.error("--check cannot be combined with a command or --native-preflight")
    if args.exec and not command:
        parser.error("--exec requires a command")
    if not args.check and not args.native_preflight and not command:
        parser.error("provide a Cargo command after --, or select --check/--native-preflight")

    try:
        environment = msvc.build_environment()
        print("MSVC environment: " + json.dumps(msvc.diagnostics(environment)), flush=True)
        msvc.preflight(environment)
        if args.native_preflight:
            msvc.native_preflight(environment, target=args.preflight_target)
        if command:
            if not args.exec:
                command.insert(0, msvc.executable("cargo", environment))
            print("Running: " + subprocess.list2cmdline(command), flush=True)
            result = subprocess.run(command, env=environment)
            return result.returncode
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Windows native build preflight failed: {error}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
