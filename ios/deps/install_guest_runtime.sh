#!/usr/bin/env bash
# Add the runtime pieces OpenCode needs to the iSH Alpine fakefs rootfs:
# libstdc++/libgcc, CA certificates, and the Linux ARM64 musl OpenCode binary.
# Runs after prepare_rootfs.sh. All downloads are cached under ios/deps/cache.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEPS="$ROOT/ios/deps"
CACHE="$DEPS/cache"
ROOTFS="$DEPS/rootfs/alpine/data"
BRANCH="${ALPINE_BRANCH:-v3.21}"
ARCH="${ALPINE_ARCH:-aarch64}"
OC_VERSION="${OPENCODE_VERSION:-v1.19.6}"
OC_URL="https://github.com/anomalyco/opencode/releases/download/$OC_VERSION/opencode-linux-arm64-musl.tar.gz"
INDEX_URL="https://dl-cdn.alpinelinux.org/alpine/$BRANCH/main/$ARCH/APKINDEX.tar.gz"

mkdir -p "$CACHE" "$ROOTFS/usr/local/bin" "$ROOTFS/etc/ssl/certs"
test -d "$ROOTFS" || { echo "ERROR: fakefs rootfs missing; run prepare_rootfs.sh first" >&2; exit 1; }

if [ ! -f "$CACHE/APKINDEX.tar.gz" ]; then
  echo "[guest] downloading Alpine package index"
  curl -fL --retry 3 -o "$CACHE/APKINDEX.tar.gz" "$INDEX_URL"
fi
mkdir -p "$CACHE/apkindex"
tar -xzf "$CACHE/APKINDEX.tar.gz" -C "$CACHE/apkindex"

python3 - "$CACHE/apkindex/APKINDEX" libstdc++ libgcc ca-certificates-bundle <<'PY' > "$CACHE/packages.txt"
import re, sys
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
  if [ ! -f "$CACHE/$pkg" ]; then
    echo "[guest] downloading $pkg"
    curl -fL --retry 3 -o "$CACHE/$pkg" \
      "https://dl-cdn.alpinelinux.org/alpine/$BRANCH/main/$ARCH/$pkg"
  fi
  echo "[guest] extracting $pkg"
  tar -xzf "$CACHE/$pkg" -C "$ROOTFS"
done < "$CACHE/packages.txt"

if [ ! -f "$CACHE/opencode.tar.gz" ]; then
  echo "[guest] downloading OpenCode Linux ARM64"
  curl -fL --retry 3 -o "$CACHE/opencode.tar.gz" "$OC_URL"
fi
rm -rf "$CACHE/opencode-extract"
mkdir -p "$CACHE/opencode-extract"
tar -xzf "$CACHE/opencode.tar.gz" -C "$CACHE/opencode-extract"
BIN="$(find "$CACHE/opencode-extract" -type f -name opencode | head -1)"
test -n "$BIN" || { echo "ERROR: OpenCode binary not found in tarball" >&2; exit 1; }
cp "$BIN" "$ROOTFS/usr/local/bin/opencode"
chmod 755 "$ROOTFS/usr/local/bin/opencode"
# Alpine's ca-certificates package places the bundle here; make sure the path
# exists for TLS clients even if the package layout changes.
if [ -f "$ROOTFS/etc/ssl/cert.pem" ] && [ ! -f "$ROOTFS/etc/ssl/certs/ca-certificates.crt" ]; then
  cp "$ROOTFS/etc/ssl/cert.pem" "$ROOTFS/etc/ssl/certs/ca-certificates.crt"
fi
echo "[guest] OpenCode runtime ready ($(du -h "$ROOTFS/usr/local/bin/opencode" | awk '{print $1}'))"