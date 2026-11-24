#!/usr/bin/env python3
"""Smoke test for tools/kimi-pty-bridge.py (runs on Linux/macOS/Termux)."""

from __future__ import annotations

import base64
import os
import re
import socket
import struct
import subprocess
import sys
import time


def recv_exact(sock: socket.socket, n: int) -> bytes:
    out = b""
    while len(out) < n:
        chunk = sock.recv(n - len(out))
        if not chunk:
            raise RuntimeError("bridge closed early")
        out += chunk
    return out


def main() -> int:
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    bridge = os.path.join(root, "tools", "kimi-pty-bridge.py")
    proc = subprocess.Popen(
        [sys.executable, bridge, "--host", "127.0.0.1", "--port", "0",
         "--token", "test-token", "--", "/bin/sh", "-c", "printf 'hello-from-bridge\\n'"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    try:
        line = proc.stdout.readline()
        print(line.strip())
        m = re.search(r":(\d+)$", line.strip())
        if not m:
            print("could not parse bridge port", file=sys.stderr)
            return 1
        port = int(m.group(1))
        s = socket.create_connection(("127.0.0.1", port), timeout=5)
        # Empty command fields => use the server's default command.
        s.sendall(b"KPTY/2 80 24 test-token  \n")
        ok = recv_exact(s, 3)
        if ok != b"OK\n":
            print("bad handshake:", ok, file=sys.stderr)
            return 1
        out = b""
        while True:
            hdr = recv_exact(s, 5)
            kind = hdr[0]
            n = struct.unpack("!I", hdr[1:])[0]
            payload = recv_exact(s, n) if n else b""
            if kind == 0:
                out += payload
            elif kind == 3:
                break
        print("output:", out.decode("utf-8", "replace").strip())
        if b"hello-from-bridge" not in out:
            return 1
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())