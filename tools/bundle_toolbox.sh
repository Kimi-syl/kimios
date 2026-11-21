#!/usr/bin/env bash
# Prebundle a real-ELF shell toolbox (dash + coreutils + their libs) into
# android/assets/toolbox.tar.gz. Extracted into the sysroot on first run so the
# TUI terminal has a working shell without a runtime apk install. Uses real
# musl ELFs because busybox *applets* die under Android's app seccomp
# (SECCOMP_RET_KILL/SIGSYS); dash/coreutils run like any other guest binary.
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

echo "[2/3] installing dash + coreutils into temp rootfs..."
LD="$ROOT/lib/ld-musl-aarch64.so.1"
V=$(cat "$ROOT/etc/alpine-release" | cut -d. -f1,2)
"$LD" --library-path "$ROOT/lib:$ROOT/usr/lib" "$ROOT/sbin/apk" --root "$ROOT" --arch aarch64 \
  --repositories-file /dev/null \
  --repository "http://dl-cdn.alpinelinux.org/alpine/v$V/main" \
  --allow-untrusted --no-progress add dash coreutils >/dev/null

echo "[3/3] assembling toolbox -> $OUT"
TB="$WORK/tb"; mkdir -p "$TB/bin" "$TB/usr/bin" "$TB/usr/lib"
cp "$ROOT/bin/coreutils" "$TB/bin/coreutils"
for f in $(ls "$ROOT/bin"); do
  if [ -L "$ROOT/bin/$f" ] && [ "$(readlink "$ROOT/bin/$f")" = "coreutils" ]; then
    cp -P "$ROOT/bin/$f" "$TB/bin/$f"
  fi
done
cp "$ROOT/usr/bin/dash" "$TB/usr/bin/dash"
for l in libacl libattr libskarnet libutmps libcrypto; do
  for f in "$ROOT"/usr/lib/$l.so*; do [ -e "$f" ] && cp -P "$f" "$TB/usr/lib/"; done
done
tar -czf "$OUT" -C "$TB" .
echo "wrote $OUT ($(stat -c%s "$OUT") B)"
