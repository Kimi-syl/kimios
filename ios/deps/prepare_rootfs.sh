#!/usr/bin/env bash
# Download an Alpine aarch64 minirootfs and convert it to iSH's fakefs layout
# (a directory containing meta.db + data/). Bundled into the app and copied to
# a writable location on first run, then mounted as the guest root by KimiISH.
set -euo pipefail

ALPINE_VER="${ALPINE_VER:-3.21.0}"
ALPINE_BRANCH="${ALPINE_BRANCH:-v3.21}"
URL="https://dl-cdn.alpinelinux.org/alpine/${ALPINE_BRANCH}/releases/aarch64/alpine-minirootfs-${ALPINE_VER}-aarch64.tar.gz"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE="$SCRIPT_DIR/cache"
OUT="$SCRIPT_DIR/rootfs/alpine"
TARBALL="$CACHE/alpine-minirootfs-aarch64.tar.gz"
FAKEFSIFY="$SCRIPT_DIR/ish/build-host/tools/fakefsify"

if [ ! -x "$FAKEFSIFY" ]; then
    echo "ERROR: $FAKEFSIFY not built — run ios/deps/build_ish.sh first"
    exit 1
fi

mkdir -p "$CACHE"
if [ ! -f "$TARBALL" ]; then
    echo "[rootfs] downloading $URL"
    curl -fL --retry 3 -o "$TARBALL" "$URL"
fi

rm -rf "$OUT"
mkdir -p "$SCRIPT_DIR/rootfs"
echo "[rootfs] fakefsify -> $OUT"
"$FAKEFSIFY" "$TARBALL" "$OUT"

echo "[rootfs] done:"
ls -la "$OUT" | head