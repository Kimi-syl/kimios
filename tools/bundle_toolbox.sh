#!/usr/bin/env bash
# Prebundle a STATIC busybox toolbox into android/assets/toolbox.tar.gz.
#
# Why static: the guest loader only maps the interpreter for the *initial* exec;
# when the shell execve()s a child, the Android kernel looks for the musl
# loader at /lib/ld-musl-aarch64.so.1 (absent on the host root) -> ENOENT.
# Statically-linked binaries have no interpreter, so the kernel runs them
# directly. A static busybox provides sh + ls/cat/cp/... in one binary; applets
# are relative symlinks to it (resolved inside sysroot/bin).
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO/android/assets/toolbox.tar.gz"
VER="${ALPINE_VER:-latest-stable}"
BASE="http://dl-cdn.alpinelinux.org/alpine/$VER"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
ROOT="$WORK/rootfs"; mkdir -p "$ROOT"

echo "[1/3] fetching Alpine minirootfs ($VER)..."
MR=$(curl -s "$BASE/releases/aarch64/" | grep -oE 'alpine-minirootfs-[0-9.]+-aarch64\.tar\.gz' | sort -u | tail -1)
curl -s -o "$WORK/mr.tgz" "$BASE/releases/aarch64/$MR"
tar -xzf "$WORK/mr.tgz" -C "$ROOT"

echo "[2/3] installing busybox-static..."
LD="$ROOT/lib/ld-musl-aarch64.so.1"
V=$(cat "$ROOT/etc/alpine-release" | cut -d. -f1,2)
"$LD" --library-path "$ROOT/lib:$ROOT/usr/lib" "$ROOT/sbin/apk" --root "$ROOT" --arch aarch64 \
  --repositories-file /dev/null \
  --repository "http://dl-cdn.alpinelinux.org/alpine/v$V/main" \
  --allow-untrusted --no-progress add busybox-static >/dev/null

BBS="$ROOT/bin/busybox.static"
[ -e "$BBS" ] || BBS=$(find "$ROOT" -name 'busybox.static' | head -1)
[ -e "$BBS" ] || { echo "busybox.static not found"; exit 1; }

echo "[3/3] assembling toolbox -> $OUT"
TB="$WORK/tb"; mkdir -p "$TB/bin" "$TB/usr/bin"
cp "$BBS" "$TB/bin/busybox"
# applet symlinks (relative -> busybox) so PATH lookups work inside the guest
for a in $("$BBS" --list); do
  [ "$a" = "busybox" ] && continue
  ln -sf busybox "$TB/bin/$a" 2>/dev/null || true
done
# ensure a shell name exists
ln -sf busybox "$TB/bin/sh"
ln -sf busybox "$TB/bin/ash"
tar -czf "$OUT" -C "$TB" .
echo "wrote $OUT ($(stat -c%s "$OUT") B), $(ls "$TB/bin" | wc -l) applets"