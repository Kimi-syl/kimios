#!/usr/bin/env bash
# Runtime proof: execute the real OpenCode Linux ARM64 musl binary inside the
# iSH ARM64 interpreter. Reuses the build/rootfs from ish-native-smoke.sh.
set -euo pipefail

WORK="${1:-$(pwd)/ios/deps/ish-native}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$WORK/src"
ROOTFS="$WORK/alpine"
ISH="$SRC/build/ish"
CACHE="$WORK/cache"
OC_VERSION="${OPENCODE_VERSION:-v1.19.6}"
OC_URL="https://github.com/anomalyco/opencode/releases/download/$OC_VERSION/opencode-linux-arm64-musl.tar.gz"
INDEX_URL="https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64/APKINDEX.tar.gz"

test -x "$ISH" || { echo "ERROR: run ish-native-smoke.sh first ($ISH missing)" >&2; exit 1; }
test -d "$ROOTFS/etc" || { echo "ERROR: Alpine rootfs missing ($ROOTFS)" >&2; exit 1; }
mkdir -p "$CACHE"

if [ ! -f "$CACHE/APKINDEX.tar.gz" ]; then
  curl -fL --retry 3 -o "$CACHE/APKINDEX.tar.gz" "$INDEX_URL"
fi
mkdir -p "$CACHE/apkindex"
tar -xzf "$CACHE/APKINDEX.tar.gz" -C "$CACHE/apkindex"
python3 - "$CACHE/apkindex/APKINDEX" libstdc++ libgcc ca-certificates-bundle <<'PY' > "$CACHE/packages.txt"
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
want = set(sys.argv[2:])
for block in text.split("\n\n"):
    fields = {}
    for line in block.splitlines():
        if len(line) > 1 and line[1] == ":":
            fields[line[0]] = line[2:]
    if fields.get("P") in want:
        print(f'{fields["P"]}-{fields["V"]}.apk')
PY
while IFS= read -r pkg; do
  [ -n "$pkg" ] || continue
  [ -f "$CACHE/$pkg" ] || curl -fL --retry 3 -o "$CACHE/$pkg" \
    "https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64/$pkg"
  tar -xzf "$CACHE/$pkg" -C "$ROOTFS"
done < "$CACHE/packages.txt"

if [ ! -f "$CACHE/opencode.tar.gz" ]; then
  echo "[opencode-smoke] downloading $OC_URL"
  curl -fL --retry 3 -o "$CACHE/opencode.tar.gz" "$OC_URL"
fi
rm -rf "$CACHE/opencode-extract"
mkdir -p "$CACHE/opencode-extract"
tar -xzf "$CACHE/opencode.tar.gz" -C "$CACHE/opencode-extract"
BIN="$(find "$CACHE/opencode-extract" -type f -name opencode | head -1)"
test -n "$BIN"
mkdir -p "$ROOTFS/usr/local/bin"
cp "$BIN" "$ROOTFS/usr/local/bin/opencode"
chmod 755 "$ROOTFS/usr/local/bin/opencode"

OUT="$WORK/opencode.out"
rm -f "$OUT"
set +e
( cd "$SRC" && ./build/ish -r "$ROOTFS" /usr/local/bin/opencode --version ) > "$OUT" 2>&1 &
PID=$!
( sleep 600; kill -9 "$PID" 2>/dev/null ) & WATCH=$!
wait "$PID"; RC=$?
kill "$WATCH" 2>/dev/null || true
set -e
printf '%s\n' "----- opencode output -----"
cat "$OUT"
printf '%s\n' "----- exit=$RC -----"
if ! grep -q 'opencode' "$OUT"; then
  echo "FAIL: OpenCode binary did not run inside iSH" >&2
  exit 1
fi
# Probe the actual interactive TUI under a host PTY. The app's KimiISH PTY uses
# the same iSH devpts path; this proves OpenCode's TUI renderer starts inside the
# guest instead of only its CLI/version path.
TUI_OUT="$WORK/opencode-tui.out"
rm -f "$TUI_OUT"
export TERM=xterm-256color
export COLORTERM=truecolor
set +e
python3 "$ROOT/ios/deps/pty_probe.py" "$TUI_OUT" "$SRC" 30 \
  ./build/ish -r "$ROOTFS" /usr/local/bin/opencode
TRC=$?
set -e
echo "----- OpenCode TUI probe exit=$TRC bytes=$(wc -c < "$TUI_OUT") -----"
head -c 1200 "$TUI_OUT" | sed -n l || true
if LC_ALL=C grep -q $'\x1b\[' "$TUI_OUT"; then
  echo "[opencode-smoke] TUI emitted ANSI control sequences: PASS"
else
  echo "[opencode-smoke] FAIL: no ANSI sequences observed; TUI did not render" >&2
  exit 1
fi
echo "[opencode-smoke] PASS"