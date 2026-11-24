#!/usr/bin/env python3
"""Kimi PTY bridge: expose a real PTY over a tiny framed TCP protocol.

This is the no-emulator path for the iPad IPA. Run it on any Linux/macOS host
(or inside Termux/Android) that has the desired CLI installed:

    python3 tools/kimi-pty-bridge.py --host 0.0.0.0 --port 7681 \
        --token change-me -- opencode

Put this on the iPad in Files -> On My iPad -> KimiOS -> disk/remote-tty.cfg:

    <server-ip>:7681 change-me

Then run `opencode` in the KimiOS Terminal. The iPad sends keystrokes and
receives terminal bytes; all guest execution happens on the bridge host.

Protocol
--------
Line handshake:  KPTY/1 <cols> <rows> <token>\n   ->  OK\n
Then frames:      type:u8, length:u32be, payload
  0 = output (server -> client)
  1 = input  (client -> server)
  2 = resize (client -> server; payload rows:u16be, cols:u16be)
  3 = exit   (server -> client; payload code:i32be)

The protocol is intentionally tiny and dependency-free. Use it on a trusted
LAN or through an SSH tunnel; token auth is not transport encryption.
"""

from __future__ import annotations

import argparse
import base64
import fcntl
import hmac
import os
import pty
import select
import shlex
import shutil
import signal
import socket
import struct
import sys
import termios
import threading
import time


def frame(kind: int, payload: bytes) -> bytes:
    return struct.pack("!BI", kind, len(payload)) + payload


def parse_frames(buf: bytearray):
    out = []
    while len(buf) >= 5:
        kind = buf[0]
        n = struct.unpack("!I", bytes(buf[1:5]))[0]
        if len(buf) < 5 + n:
            break
        payload = bytes(buf[5:5 + n])
        del buf[:5 + n]
        out.append((kind, payload))
    return out


def set_winsize(fd: int, rows: int, cols: int) -> None:
    rows = max(1, min(rows, 65535))
    cols = max(1, min(cols, 65535))
    # TIOCSWINSZ makes the tty driver deliver SIGWINCH to the foreground
    # process group; no explicit kill is needed.
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def handle(client: socket.socket, addr, command, token: str) -> None:
    print(f"[bridge] client {addr[0]}:{addr[1]}", flush=True)
    client.settimeout(10)
    hello = bytearray()
    try:
        while b"\n" not in hello and len(hello) < 4096:
            chunk = client.recv(4096)
            if not chunk:
                return
            hello.extend(chunk)
    except OSError:
        return
    if b"\n" not in hello:
        return
    line, rest = hello.split(b"\n", 1)
    parts = line.decode("utf-8", "replace").strip().split()
    if len(parts) < 3 or parts[0] not in ("KPTY/1", "KPTY/2"):
        client.sendall(b"ERR handshake\n")
        return
    try:
        cols = int(parts[1])
        rows = int(parts[2])
    except ValueError:
        client.sendall(b"ERR size\n")
        return
    supplied = parts[3] if len(parts) > 3 else ""
    if token and not hmac.compare_digest(supplied, token):
        client.sendall(b"ERR auth\n")
        return

    client_command = command
    if parts[0] == "KPTY/2" and len(parts) >= 6:
        try:
            req_bin = base64.b64decode(parts[4]).decode("utf-8")
            req_arg = base64.b64decode(parts[5]).decode("utf-8")
        except Exception:
            client.sendall(b"ERR command\n")
            return
        if req_bin:
            argv = [req_bin] + shlex.split(req_arg)
            if "/" not in req_bin:
                resolved = shutil.which(req_bin)
                if not resolved:
                    client.sendall(f"ERR command not found: {req_bin}\n".encode())
                    return
                argv[0] = resolved
            client_command = argv

    env = os.environ.copy()
    env.setdefault("TERM", "xterm-256color")
    env.setdefault("COLORTERM", "truecolor")
    pid, master = pty.fork()
    if pid == 0:
        # Child: own session + controlling tty is set up by forkpty.
        try:
            os.execvpe(client_command[0], client_command, env)
        except Exception as exc:  # noqa: BLE001
            os.write(2, f"kimi-pty-bridge: cannot exec {client_command[0]}: {exc}\n".encode())
            os._exit(127)

    try:
        set_winsize(master, rows, cols)
    except OSError:
        pass
    client.sendall(b"OK\n")
    client.setblocking(False)
    inbuf = bytearray(rest)
    outbuf = b""
    exit_code = None
    last_alive = time.monotonic()

    try:
        while True:
            # Drain any complete client frames already buffered.
            for kind, payload in parse_frames(inbuf):
                if kind == 1:
                    try:
                        os.write(master, payload)
                    except OSError:
                        pass
                elif kind == 2 and len(payload) >= 4:
                    r, c = struct.unpack("!HH", payload[:4])
                    try:
                        set_winsize(master, r, c)
                    except OSError:
                        pass
                elif kind == 3:
                    return

            rlist = [master, client]
            try:
                readable, _, _ = select.select(rlist, [], [], 0.05)
            except (OSError, ValueError):
                break

            if master in readable:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    data = b""
                if data:
                    outbuf += frame(0, data)
                else:
                    break

            if client in readable:
                try:
                    data = client.recv(65536)
                except BlockingIOError:
                    data = b""
                except OSError:
                    break
                if not data:
                    break
                inbuf.extend(data)

            if outbuf:
                try:
                    client.sendall(outbuf)
                    outbuf = b""
                except OSError:
                    break

            wpid, status = os.waitpid(pid, os.WNOHANG)
            if wpid == pid:
                exit_code = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic() - last_alive > 5:
                last_alive = time.monotonic()

        # Drain the last PTY bytes before reporting exit.
        while True:
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            if not data:
                break
            try:
                client.sendall(frame(0, data))
            except OSError:
                break
        if exit_code is None:
            try:
                _, status = os.waitpid(pid, 0)
                exit_code = os.waitstatus_to_exitcode(status)
            except OSError:
                exit_code = 1
        try:
            client.sendall(frame(3, struct.pack("!i", int(exit_code))))
        except OSError:
            pass
    finally:
        try:
            os.close(master)
        except OSError:
            pass
        try:
            os.kill(pid, signal.SIGHUP)
        except OSError:
            pass
        try:
            os.waitpid(pid, os.WNOHANG)
        except OSError:
            pass
        try:
            client.close()
        except OSError:
            pass
        print(f"[bridge] client {addr[0]}:{addr[1]} closed", flush=True)


def main() -> int:
    ap = argparse.ArgumentParser(description="Kimi PTY bridge for the iPad IPA")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=7681)
    ap.add_argument("--token", default=os.environ.get("KIMI_PTY_TOKEN", ""))
    ap.add_argument("command", nargs=argparse.REMAINDER,
                    help="command to run (default: /bin/sh -l); use -- opencode")
    args = ap.parse_args()
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        command = ["/bin/sh", "-l"]

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.host, args.port))
    srv.listen(8)
    actual_port = srv.getsockname()[1]
    print(f"[bridge] listening on {args.host}:{actual_port}", flush=True)
    print(f"[bridge] command: {' '.join(command)}", flush=True)
    if not args.token:
        print("[bridge] WARNING: no --token; use only on a trusted network", flush=True)
    try:
        while True:
            client, addr = srv.accept()
            threading.Thread(target=handle, args=(client, addr, command, args.token),
                             daemon=True).start()
    except KeyboardInterrupt:
        return 0
    finally:
        srv.close()


if __name__ == "__main__":
    raise SystemExit(main())