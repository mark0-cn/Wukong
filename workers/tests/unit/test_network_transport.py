import socket
import unittest
from unittest.mock import Mock
from worker_network_evidence import Transcript
from worker_network_transport import ReplayConnection


class TransportTests(unittest.TestCase):
    def test_partial_send_fragmented_delimiter_and_stale_buffer(self):
        stream = Mock()
        stream.send.side_effect = [socket.timeout(), 1, 2, 1]
        stream.recv.side_effect = [b"ready\r", b"\nold\n", b"new\n"]
        transcript = Transcript()
        transport = ReplayConnection(stream, transcript, lambda: None)
        self.assertEqual(transport.receive(0, b"\r\n", 64), (b"ready\r\n", b"ready\r\n"))
        transport.send(1, b"PING")
        self.assertEqual(transport.receive(2, b"\n", 64), (b"old\n", b""))
        self.assertEqual(transport.receive(3, b"\n", 64), (b"new\n", b"new\n"))
        self.assertEqual(transport.count, len(b"ready\r\nold\nnew\nPING"))

    def test_failed_send_keeps_only_bytes_actually_written(self):
        stream = Mock()
        stream.send.side_effect = [2, 0]
        transcript = Transcript()
        transport = ReplayConnection(stream, transcript, lambda: None)
        with self.assertRaises(EOFError):
            transport.send(0, b"PING")
        self.assertEqual(transcript.steps[0]["data_base64"], "UEk=")
        self.assertEqual(transport.count, 2)
