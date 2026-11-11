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
from worker_common import BRIDGE_PATH, MAX_RPC, PROTOCOL, atomic_json, load_json, rpc_error, safe_env
from worker_processes import detached_options
from worker_runtime import Runtime

def serve(root, trusted):
    runtime = Runtime(root, trusted)
    token = secrets.token_hex(32)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            self.connection.settimeout(310)
            try:
                line = self.rfile.readline(MAX_RPC + 1)
                if len(line) > MAX_RPC:
                    raise ValueError("request exceeds size limit")
                request = json.loads(line)
                if not secrets.compare_digest(str(request.pop("auth", "")), token):
                    raise ValueError("worker authentication failed")
                result = runtime.dispatch(request["method"], request.get("params") or {})
                response = {"jsonrpc": "2.0", "id": request.get("id"), "result": result}
            except Exception as error:
                response = {"jsonrpc": "2.0", "id": locals().get("request", {}).get("id"), "error": rpc_error(error)}
            encoded = json.dumps(response, ensure_ascii=False).encode() + b"\n"
            if len(encoded) > MAX_RPC:
                encoded = json.dumps({"jsonrpc": "2.0", "id": response.get("id"), "error": {"code": -32001, "message": "response exceeds size limit"}}).encode() + b"\n"
            self.wfile.write(encoded)
            if locals().get("request", {}).get("method") == "worker.shutdown" and "error" not in response:
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
    request = {**request, "auth": endpoint["auth"]}
    with socket.create_connection(("127.0.0.1", int(endpoint["port"])), timeout=min(5, timeout)) as connection:
        connection.settimeout(timeout)
        connection.sendall(json.dumps(request).encode() + b"\n")
        with connection.makefile("rb") as stream:
            line = stream.readline(MAX_RPC + 1)
            if not line or len(line) > MAX_RPC:
                raise ValueError("invalid daemon response")
            return json.loads(line)

def ensure_daemon(root, trusted):
    endpoint_path = root / "endpoint.json"

    def existing():
        try:
            endpoint = load_json(endpoint_path)
            reply = relay_call(endpoint, {"id": 0, "method": "ping"}, timeout=1)
            if reply.get("result", {}).get("protocol") == PROTOCOL:
                return endpoint
        except (OSError, ValueError, KeyError, socket.timeout):
            pass
        return None

    endpoint = existing()
    if endpoint:
        return endpoint
    # Serialize daemon startup across relays. A stale lock is only removed after
    # its age exceeds startup allowance and a fresh endpoint probe still fails.
    lock = root / "startup.lock"
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            endpoint = existing()
            if endpoint:
                return endpoint
            if time.time() - lock.stat().st_mtime > 45:
                try:
                    lock.rmdir()
                except OSError:
                    pass
            time.sleep(0.1)
    else:
        raise ValueError("worker daemon startup lock timeout")
    try:
        endpoint = existing()
        if endpoint:
            return endpoint
        argv = [sys.executable, str(BRIDGE_PATH), "--state", str(root), "--daemon"]
        if trusted:
            argv.append("--trusted-local")
        with (root / "daemon.log").open("ab") as log:
            process = subprocess.Popen(argv, env=safe_env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log, **detached_options())
        # A cold Windows boot-identity query alone can take up to 15 seconds.
        # Keep the startup lock while that bounded initialization is in flight.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            endpoint = existing()
            if endpoint:
                return endpoint
            if process.poll() is not None:
                raise ValueError(f"worker daemon exited during startup ({process.returncode}); inspect daemon.log")
            time.sleep(0.05)
        raise ValueError("worker daemon failed to start; inspect daemon.log")
    finally:
        try:
            lock.rmdir()
        except OSError:
            pass
