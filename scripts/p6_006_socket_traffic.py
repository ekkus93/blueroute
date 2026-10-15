#!/usr/bin/env python3
"""P6-006 TCP/UDP socket acceptance probe using only the Python standard library."""

from __future__ import annotations

import argparse
import hashlib
import socket
import struct
import sys
import time

TCP_PORT = 46006
UDP_PORT = 46007
BLOCK = bytes(range(256)) * 256


def tcp_server(bind: str, port: int) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((bind, port))
        listener.listen(1)
        print(f"TCP listening on {bind}:{port}", flush=True)
        conn, peer = listener.accept()
        with conn:
            digest = hashlib.sha256()
            received = 0
            while True:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                digest.update(chunk)
                received += len(chunk)
            summary = f"{received} {digest.hexdigest()}\n".encode()
            conn.sendall(summary)
    print(
        f"P6-006 TCP server PASS peer={peer[0]} bytes={received} sha256={digest.hexdigest()}",
        flush=True,
    )


def tcp_client(host: str, port: int, byte_count: int) -> None:
    digest = hashlib.sha256()
    sent = 0
    with socket.create_connection((host, port), timeout=10) as conn:
        while sent < byte_count:
            chunk = BLOCK[: min(len(BLOCK), byte_count - sent)]
            conn.sendall(chunk)
            digest.update(chunk)
            sent += len(chunk)
        conn.shutdown(socket.SHUT_WR)
        summary = b""
        while not summary.endswith(b"\n"):
            chunk = conn.recv(256)
            if not chunk:
                break
            summary += chunk
    try:
        remote_count_text, remote_digest = summary.decode().strip().split(" ", 1)
        remote_count = int(remote_count_text)
    except (UnicodeDecodeError, ValueError) as error:
        raise RuntimeError(f"invalid TCP server summary: {summary!r}") from error
    local_digest = digest.hexdigest()
    if remote_count != sent or remote_digest != local_digest:
        raise RuntimeError(
            "TCP integrity mismatch: "
            f"sent={sent} remote={remote_count} "
            f"local_sha256={local_digest} remote_sha256={remote_digest}"
        )
    print(f"P6-006 TCP client PASS bytes={sent} sha256={local_digest}")


def udp_server(bind: str, port: int, count: int, payload_size: int, timeout: float) -> None:
    if payload_size < 1:
        raise ValueError("UDP payload size must be positive")
    seen: set[int] = set()
    peer: tuple[str, int] | None = None
    deadline = time.monotonic() + timeout
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind((bind, port))
        sock.settimeout(0.25)
        print(
            f"UDP listening on {bind}:{port} count={count} payload={payload_size}",
            flush=True,
        )
        while len(seen) < count and time.monotonic() < deadline:
            try:
                packet, current_peer = sock.recvfrom(payload_size + 4)
            except TimeoutError:
                continue
            if len(packet) != payload_size + 4:
                raise RuntimeError(f"unexpected UDP datagram length {len(packet)}")
            sequence = struct.unpack("!I", packet[:4])[0]
            if sequence >= count:
                raise RuntimeError(f"unexpected UDP sequence {sequence}")
            expected = bytes([sequence % 251]) * payload_size
            if packet[4:] != expected:
                raise RuntimeError(f"UDP payload mismatch for sequence {sequence}")
            seen.add(sequence)
            peer = current_peer
        missing = count - len(seen)
        if peer is not None:
            sock.sendto(f"received={len(seen)} missing={missing}".encode(), peer)
    if missing:
        raise RuntimeError(f"UDP loss: received={len(seen)} missing={missing}")
    print(f"P6-006 UDP server PASS received={len(seen)} missing=0", flush=True)


def udp_client(
    host: str,
    port: int,
    count: int,
    payload_size: int,
    interval: float,
    timeout: float,
) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.connect((host, port))
        sock.settimeout(timeout)
        for sequence in range(count):
            payload = bytes([sequence % 251]) * payload_size
            sock.send(struct.pack("!I", sequence) + payload)
            if interval:
                time.sleep(interval)
        summary = sock.recv(256).decode()
    expected = f"received={count} missing=0"
    if summary != expected:
        raise RuntimeError(f"UDP integrity/loss check failed: {summary!r}, expected {expected!r}")
    print(f"P6-006 UDP client PASS sent={count} payload={payload_size} missing=0")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    tcp_s = commands.add_parser("tcp-server")
    tcp_s.add_argument("--bind", default="0.0.0.0")
    tcp_s.add_argument("--port", type=int, default=TCP_PORT)

    tcp_c = commands.add_parser("tcp-client")
    tcp_c.add_argument("host")
    tcp_c.add_argument("--port", type=int, default=TCP_PORT)
    tcp_c.add_argument("--bytes", type=int, default=16 * 1024 * 1024)

    udp_s = commands.add_parser("udp-server")
    udp_s.add_argument("--bind", default="0.0.0.0")
    udp_s.add_argument("--port", type=int, default=UDP_PORT)
    udp_s.add_argument("--count", type=int, default=256)
    udp_s.add_argument("--payload", type=int, default=1024)
    udp_s.add_argument("--timeout", type=float, default=5.0)

    udp_c = commands.add_parser("udp-client")
    udp_c.add_argument("host")
    udp_c.add_argument("--port", type=int, default=UDP_PORT)
    udp_c.add_argument("--count", type=int, default=256)
    udp_c.add_argument("--payload", type=int, default=1024)
    udp_c.add_argument("--interval", type=float, default=0.002)
    udp_c.add_argument("--timeout", type=float, default=10.0)
    return root


def main() -> int:
    args = parser().parse_args()
    if args.command == "tcp-server":
        tcp_server(args.bind, args.port)
    elif args.command == "tcp-client":
        tcp_client(args.host, args.port, args.bytes)
    elif args.command == "udp-server":
        udp_server(args.bind, args.port, args.count, args.payload, args.timeout)
    elif args.command == "udp-client":
        udp_client(args.host, args.port, args.count, args.payload, args.interval, args.timeout)
    else:
        raise AssertionError(args.command)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"P6-006 socket probe FAIL: {error}", file=sys.stderr)
        sys.exit(1)
