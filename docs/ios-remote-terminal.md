# Remote terminal backend for the iPad IPA

The IPA cannot locally `fork`/`exec` Linux ARM64 binaries. Rather than shipping
a large emulator, KimiOS uses a **tiny remote PTY bridge**: a separate host owns
the real PTY and runs `opencode`/`vim`/`htop`/a shell; the iPad transports bytes
and renders them with the shared VT emulator.

No emulator, no VM, and no downloaded code executes on iOS.

## 1. Start the bridge on a host

On Linux/macOS, or in Termux on Android:

```bash
python3 tools/kimi-pty-bridge.py \
  --host 0.0.0.0 --port 7681 --token change-me \
  -- opencode
```

Use `/bin/sh -l` instead of `-- opencode` for a general shell:

```bash
python3 tools/kimi-pty-bridge.py --host 0.0.0.0 --port 7681 --token change-me
```

Security: the transport is plain TCP with a bearer token, not TLS. Use a
trusted LAN, a VPN, or an SSH tunnel:

```bash
ssh -L 7681:127.0.0.1:7681 user@server
```

Then the iPad can use `127.0.0.1:7681` only if the tunnel is on the iPad (not
possible with a normal desktop SSH client); for a direct LAN bridge use the
server's LAN address.

## 2. Configure the IPA

The IPA enables Files sharing. In **Files → On My iPad → KimiOS → disk**, create
`remote-tty.cfg`:

```text
192.168.1.50:7681 change-me
```

Alternatively set `KIMI_REMOTE_TTY=192.168.1.50:7681 change-me` in the host
harness environment.

## 3. Run a TUI

Open Terminal and type the bridge command, e.g.:

```text
opencode
```

The terminal's `SYS_PROC` implementation connects to the bridge, sends terminal
size/keystrokes and renders the bridge's PTY output. The OpenCode TUI therefore
works on iPad without a local emulator.

## Protocol

```text
KPTY/1 <cols> <rows> <token>\n  ->  OK\n

frame: type:u8, length:u32be, payload
  0 = output        server -> client
  1 = input         client -> server
  2 = resize        client -> server, payload rows:u16be cols:u16be
  3 = exit          server -> client, payload code:i32be
```

The bridge is intentionally dependency-free (Python standard library) and runs
on Linux/macOS/Termux. The iOS client is `ios/app/Sources/KimiRemoteTTY.swift`.