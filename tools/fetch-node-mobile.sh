#!/usr/bin/env bash
# Fetch the Node 24.5 iOS runtime used by the local OpenCode server.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/ios/app/Vendor/NodeMobile.xcframework"
BUILD="$ROOT/ios/build"
ZIP="$BUILD/nodejs-mobile-ios.zip"
URL="https://github.com/funnycups/nodejs-mobile/releases/download/v1.0.3/nodejs-mobile-ios.zip"
SHA256="761a26d88547cc8adc5e73d18c312e04dd08eaa585c0f6e8a079298f3ec7d2f0"

if [ -d "$DEST" ]; then
  echo "[node] NodeMobile.xcframework already present"
  exit 0
fi

mkdir -p "$BUILD" "$(dirname "$DEST")"
if [ ! -f "$ZIP" ]; then
  echo "[node] downloading NodeMobile.xcframework"
  curl -fL --retry 3 -o "$ZIP" "$URL"
fi
ACTUAL="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
if [ "$ACTUAL" != "$SHA256" ]; then
  echo "ERROR: NodeMobile checksum mismatch: $ACTUAL" >&2
  exit 1
fi
rm -rf "$BUILD/nodejs-mobile-unzip"
mkdir -p "$BUILD/nodejs-mobile-unzip"
unzip -q "$ZIP" -d "$BUILD/nodejs-mobile-unzip"
cp -R "$BUILD/nodejs-mobile-unzip/NodeMobile.xcframework" "$DEST"
echo "[node] NodeMobile.xcframework ready at $DEST"