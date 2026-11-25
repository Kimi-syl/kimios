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
# Probe the actual interactive TUI through tmux, which (unlike a bare pipe)
# answers OpenCode's terminal capability queries and provides a real screen.
TUI_OUT="$WORK/opencode-tui.out"
rm -f "$TUI_OUT"
export TERM=xterm-256color
export COLORTERM=truecolor
tmux kill-session -t kimios-oc 2>/dev/null || true
tmux new-session -d -s kimios-oc -x 100 -y 30 \
  "cd '$SRC' && exec ./build/ish -r '$ROOTFS' /usr/local/bin/opencode"
sleep 45
tmux capture-pane -p -t kimios-oc -S -40 > "$TUI_OUT" 2>&1 || true
tmux kill-session -t kimios-oc 2>/dev/null || true
echo "----- OpenCode TUI pane ($(wc -l < "$TUI_OUT") lines, $(wc -c < "$TUI_OUT") bytes) -----"
sed -n '1,80p' "$TUI_OUT" || true
# A TUI frame must contain actual UI text; ANSI alone can be a startup query.
if [ "$(tr -d '[:space:]' < "$TUI_OUT" | wc -c)" -lt 40 ]; then
  echo "FAIL: OpenCode TUI pane is empty" >&2
  exit 1
fi
if grep -qiE 'error|exec format|not found' "$TUI_OUT"; then
  echo "FAIL: OpenCode TUI reported an error" >&2
  exit 1
fi
echo "[opencode-smoke] PASS"