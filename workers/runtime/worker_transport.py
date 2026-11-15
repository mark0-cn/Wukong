"""Authenticated daemon transport and stdio relay lifecycle."""
import json
import os
import secrets
import socket
import socketserver
import subprocess
import sys
import threading
import time
from filelock import FileLock
from worker_common import BRIDGE_PATH, MAX_RPC, PROTOCOL, atomic_json, load_json, safe_env, tail
from worker_processes import detached_options
from worker_runtime import Runtime
from worker_rpc import Server as RpcServer

def serve(root, trusted):
    # The daemon owns this lock independently of a relay that may crash.
    with FileLock(root / "daemon.guard", timeout=0):
        runtime = Runtime(root, trusted)
        token = secrets.token_hex(32)
        rpc = RpcServer(runtime)

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                self.connection.settimeout(310)
                auth = self.rfile.readline(66).strip()
                if not secrets.compare_digest(auth, token.encode("ascii")):
                    return
                line = self.rfile.readline(MAX_RPC + 1)
                if not line or len(line) > MAX_RPC:
                    return
                response = rpc.call(line.decode("utf-8"))
                encoded = response.encode("utf-8") + b"\n"
                if len(encoded) > MAX_RPC:
                    return  # uncertain transport outcome; reconcile the durable request
                self.wfile.write(encoded)
                if runtime.stopping:
                    threading.Thread(target=self.server.shutdown, daemon=True).start()

        class Server(socketserver.ThreadingTCPServer):
            daemon_threads = True
            allow_reuse_address = False

        with Server(("127.0.0.1", 0), Handler) as server:
            endpoint = {"protocol": PROTOCOL, "host": "127.0.0.1", "port": server.server_address[1], "auth": token, "pid": os.getpid()}
            atomic_json(root / "endpoint.json", endpoint)
            (root / "endpoint.json").chmod(0o600)
            server.serve_forever(poll_interval=0.25)

def relay_call(endpoint, request, timeout=305):
    raw = request if isinstance(request, bytes) else json.dumps(request).encode("utf-8")
    with socket.create_connection(("127.0.0.1", int(endpoint["port"])), timeout=min(5, timeout)) as connection:
        connection.settimeout(timeout)
        connection.sendall(endpoint["auth"].encode("ascii") + b"\n" + raw.rstrip(b"\n") + b"\n")
        with connection.makefile("rb") as stream:
            line = stream.readline(MAX_RPC + 1)
            if not line or len(line) > MAX_RPC:
                raise ValueError("invalid daemon response")
            return line if isinstance(request, bytes) else json.loads(line)

def ensure_daemon(root, trusted):
    endpoint_path = root / "endpoint.json"

    def existing():
        try:
            endpoint = load_json(endpoint_path)
            reply = relay_call(endpoint, {"jsonrpc": "2.0", "id": 0, "method": "ping"}, timeout=1)
            if reply.get("result", {}).get("protocol") == PROTOCOL:
                return endpoint
        except (OSError, ValueError, KeyError, socket.timeout):
            pass
        return None

    endpoint = existing()
    if endpoint:
        return endpoint
    # Kernel-backed ownership survives slow initialization and is released on
    # process exit. Never infer ownership from a directory's wall-clock age.
    with FileLock(root / "startup.guard", timeout=30):
        endpoint = existing()
        if endpoint:
            return endpoint
        argv = [sys.executable, str(BRIDGE_PATH), "--state", str(root), "--daemon"]
        if trusted:
            argv.append("--trusted-local")
        with (root / "daemon.log").open("ab") as log:
            process = subprocess.Popen(argv, env=safe_env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log, **detached_options())
        # Cold interpreter imports and state initialization can outlast a warm
        # reconnect. Keep the startup lock across the bounded readiness wait.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            endpoint = existing()
            if endpoint:
                return endpoint
            if process.poll() is not None:
                try:
                    diagnostic = tail(root / "daemon.log", 2048)
                except OSError:
                    diagnostic = "local startup log could not be read"
                raise ValueError(f"worker daemon exited during startup ({process.returncode}); daemon.log tail: {diagnostic}")
            time.sleep(0.05)
        raise ValueError("worker daemon failed to start; inspect daemon.log")
