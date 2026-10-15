"""Loopback-only P6-006 TCP/UDP smoke tests.

These tests use real IPv4 localhost sockets, not Bluetooth PAN, BlueZ,
NetworkManager, RF isolation, multiple computers, or production JoinNetwork.
"""
from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
import selectors
import socket
import subprocess
import sys
import unittest

import p6_006_socket_traffic as traffic

SCRIPT = Path(__file__).with_name("p6_006_socket_traffic.py")


@contextmanager
def probe_server(protocol: str, *options: str):
    kind = socket.SOCK_STREAM if protocol == "tcp-server" else socket.SOCK_DGRAM
    with socket.socket(socket.AF_INET, kind) as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]

    process = subprocess.Popen(
        [sys.executable, str(SCRIPT), protocol, "--bind", "127.0.0.1",
         "--port", str(port), *options],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    try:
        assert process.stdout is not None
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            if not selector.select(timeout=10):
                raise AssertionError(f"{protocol} did not report readiness")
            ready = process.stdout.readline()
        if "listening" not in ready:
            raise AssertionError(f"{protocol} failed to start: {ready!r}")
        yield port
        output, errors = process.communicate(timeout=15)
        if process.returncode != 0:
            raise AssertionError(
                f"{protocol} exited with {process.returncode}: {errors}"
            )
        if " PASS " not in output:
            raise AssertionError(f"{protocol} missing server integrity PASS: {output}")
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)


class LoopbackTrafficTests(unittest.TestCase):
    def test_tcp_real_loopback_integrity(self):
        # Deliberately not a multiple of the transfer block size.
        with probe_server("tcp-server") as port:
            traffic.tcp_client("127.0.0.1", port, 131071)

    def test_udp_real_loopback_sequences_and_integrity(self):
        with probe_server(
            "udp-server", "--count", "24", "--payload", "144",
            "--timeout", "5",
        ) as port:
            traffic.udp_client("127.0.0.1", port, 24, 144, 0.005, 5)


if __name__ == "__main__":
    unittest.main()
