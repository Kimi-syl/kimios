#!/usr/bin/env python3
"""Run a command on a real PTY and capture its terminal bytes for N seconds."""
import fcntl, os, pty, select, signal, struct, subprocess, sys, termios, time

out_path, cwd, seconds = sys.argv[1], sys.argv[2], float(sys.argv[3])
argv = sys.argv[4:]
master, slave = pty.openpty()
# 80x24 xterm
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
env = os.environ.copy()
env["TERM"] = env.get("TERM", "xterm-256color")
env["COLORTERM"] = "truecolor"
p = subprocess.Popen(argv, cwd=cwd, stdin=slave, stdout=slave, stderr=slave,
                     env=env, preexec_fn=os.setsid)
os.close(slave)
os.set_blocking(master, False)
data = bytearray()
deadline = time.time() + seconds
try:
    while time.time() < deadline:
        r, _, _ = select.select([master], [], [], 0.25)
        if r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            data.extend(chunk)
finally:
    try:
        os.killpg(os.getpgid(p.pid), signal.SIGKILL)
    except OSError:
        pass
    try:
        p.wait(timeout=2)
    except Exception:
        pass
    os.close(master)
open(out_path, "wb").write(data)
print(f"pty probe captured {len(data)} bytes, exit={p.returncode}", file=sys.stderr)
