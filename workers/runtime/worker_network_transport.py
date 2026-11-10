"""Bounded TCP framing, partial I/O and already-buffered response provenance."""
import base64
import socket
from worker_network_plan import LIMIT


class ReplayConnection:
    def __init__(self, socket, transcript, check):
        self.socket = socket
        self.transcript = transcript
        self.check = check
        self.pending = bytearray()
        self.stale = 0
        self.count = 0

    def send(self, index, data):
        event = self.transcript.step(index, "send")
        written = 0
        try:
            while written < len(data):
                self.check()
                if self.count + len(data) - written > LIMIT or len(self.transcript.events) >= 4096:
                    raise ValueError("transcript budget exceeded")
                try:
                    amount = self.socket.send(data[written:])
                except socket.timeout:
                    continue
                if not amount:
                    raise EOFError("connection closed during send")
                self.transcript.wire(index, "send", data[written:written+amount])
                written += amount
                self.count += amount
        finally:
            event["data_base64"] = base64.b64encode(data[:written]).decode()
        self.stale = len(self.pending)

    def receive(self, index, delimiter, maximum):
        data, fresh = bytearray(), bytearray()
        event = self.transcript.step(index, "receive")

        def consume(amount):
            stale = min(amount, self.stale)
            data.extend(self.pending[:amount])
            fresh.extend(self.pending[stale:amount])
            del self.pending[:amount]
            self.stale -= stale

        try:
            while True:
                self.check()
                found = self.pending.find(delimiter)
                if found >= 0:
                    take = found + len(delimiter)
                    if len(data) + take > maximum:
                        raise ValueError("receive step byte budget exceeded")
                    consume(take)
                    return bytes(data), bytes(fresh)
                if self.pending:
                    consume(max(0, len(self.pending)-len(delimiter)+1))
                if len(data)+len(self.pending) >= maximum or self.count >= LIMIT or len(self.transcript.events) >= 4096:
                    raise ValueError("receive/transcript budget exceeded")
                try:
                    chunk = self.socket.recv(min(4096, maximum-len(data)-len(self.pending), LIMIT-self.count))
                except socket.timeout:
                    continue
                if not chunk:
                    raise EOFError("EOF before delimiter")
                self.transcript.wire(index, "receive", chunk)
                self.pending.extend(chunk)
                self.count += len(chunk)
        finally:
            event["data_base64"] = base64.b64encode(data).decode()
