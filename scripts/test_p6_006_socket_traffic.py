"""Deterministic software-only contract tests for P6-006 socket probes.

The sockets are mocked. Passing these tests never establishes a Bluetooth PAN
link, independent DNS operation, or actual two-machine traffic acceptance.
"""
from __future__ import annotations

import hashlib
import socket
import struct
import unittest
from unittest.mock import patch

import p6_006_socket_traffic as probe


class FakeTcpConnection:
    def __init__(self, *, corrupt: bool = False):
        self.sent = bytearray()
        self.corrupt = corrupt
        self.summary = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def sendall(self, data):
        self.sent.extend(data)

    def shutdown(self, how):
        assert how == socket.SHUT_WR
        digest = hashlib.sha256(self.sent).hexdigest()
        if self.corrupt:
            digest = "0" * 64
        self.summary = f"{len(self.sent)} {digest}\n".encode()

    def recv(self, _length):
        data, self.summary = self.summary, b""
        return data


class FakeUdpConnection:
    def __init__(self, summary):
        self.destination = None
        self.timeout = None
        self.packets = []
        self.summary = summary

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def connect(self, destination):
        self.destination = destination

    def settimeout(self, timeout):
        self.timeout = timeout

    def send(self, packet):
        self.packets.append(packet)

    def recv(self, _length):
        return self.summary


class FakeUdpListener:
    def __init__(self, packets):
        self.packets = list(packets)
        self.responses = []

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def bind(self, address):
        self.address = address

    def settimeout(self, value):
        self.timeout = value

    def recvfrom(self, _length):
        item = self.packets.pop(0)
        if isinstance(item, tuple):
            return item
        return item, ("127.0.0.1", 12345)

    def sendto(self, data, peer):
        self.responses.append((data, peer))


class P6006SocketContractTests(unittest.TestCase):
    def test_tcp_transfer_sha256_is_checked_for_non_aligned_size(self):
        connection = FakeTcpConnection()
        with patch.object(probe.socket, "create_connection", return_value=connection):
            probe.tcp_client("127.0.0.1", probe.TCP_PORT, 131071)
        self.assertEqual(len(connection.sent), 131071)
        self.assertEqual(connection.sent[:256], bytes(range(256)))

    def test_tcp_remote_integrity_mismatch_is_a_failure(self):
        connection = FakeTcpConnection(corrupt=True)
        with patch.object(probe.socket, "create_connection", return_value=connection):
            with self.assertRaisesRegex(RuntimeError, "TCP integrity mismatch"):
                probe.tcp_client("127.0.0.1", probe.TCP_PORT, 70003)

    def test_udp_client_sequences_payloads_and_requires_success_summary(self):
        count, size = 4, 32
        connection = FakeUdpConnection(f"received={count} missing=0".encode())
        with patch.object(probe.socket, "socket", return_value=connection):
            probe.udp_client("127.0.0.1", probe.UDP_PORT, count, size, 0, 1)
        self.assertEqual(connection.destination, ("127.0.0.1", probe.UDP_PORT))
        for sequence, packet in enumerate(connection.packets):
            self.assertEqual(struct.unpack("!I", packet[:4])[0], sequence)
            self.assertEqual(packet[4:], bytes([sequence % 251]) * size)

        connection = FakeUdpConnection(b"received=3 missing=1")
        with patch.object(probe.socket, "socket", return_value=connection):
            with self.assertRaisesRegex(RuntimeError, "UDP integrity/loss check failed"):
                probe.udp_client("127.0.0.1", probe.UDP_PORT, 4, 32, 0, 1)

    def test_udp_listener_rejects_cross_peer_splicing_as_false_success(self):
        payload = lambda seq: struct.pack("!I", seq) + bytes([seq % 251]) * 8
        listener = FakeUdpListener([
            (payload(0), ("127.0.0.1", 12345)),
            (payload(1), ("127.0.0.1", 54321)),
        ])
        with patch.object(probe.socket, "socket", return_value=listener):
            with self.assertRaisesRegex(RuntimeError, "multiple sender endpoints"):
                probe.udp_server("127.0.0.1", probe.UDP_PORT, 2, 8, 1.0)
        self.assertEqual(listener.responses, [])

    def test_udp_listener_detects_corrupted_payload_and_reports_valid_delivery(self):
        packet = lambda seq: struct.pack("!I", seq) + bytes([seq % 251]) * 8
        listener = FakeUdpListener([packet(0), packet(1), packet(2)])
        with patch.object(probe.socket, "socket", return_value=listener):
            probe.udp_server("127.0.0.1", probe.UDP_PORT, 3, 8, 1.0)
        self.assertEqual(listener.responses[0][0], b"received=3 missing=0")

        listener = FakeUdpListener([struct.pack("!I", 0) + b"invalid!"])
        with patch.object(probe.socket, "socket", return_value=listener):
            with self.assertRaisesRegex(RuntimeError, "UDP payload mismatch"):
                probe.udp_server("127.0.0.1", probe.UDP_PORT, 1, 8, 1.0)


if __name__ == "__main__":
    unittest.main()
