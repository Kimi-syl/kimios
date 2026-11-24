#!/usr/bin/env bash
# Build the Node-target OpenCode bundle for the iOS app.
#
# The iPad runs the OpenCode server in NodeMobile (Node 24.5). This script
# produces a self-contained opencode.mjs + web assets and patches the two native
# optional dependencies (node-pty, parcel/watcher) to use iOS shims.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/ios/app/Resources/opencode"
BUILD="$ROOT/ios/build"
SRC="${KIMI_OPENCODE_SRC:-$BUILD/opencode-src}"
PIN="81045bf4f85258710a400899a29e2b6aa0213e4a"

if [ -f "$OUT/opencode.mjs" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "[opencode] bundle already present"
  exit 0
fi

mkdir -p "$BUILD" "$OUT"

# --- Bun (OpenCode's build tool) -------------------------------------------
BUN="${BUN:-}"
if [ -z "$BUN" ]; then
  if command -v bun >/dev/null 2>&1; then
    BUN="$(command -v bun)"
  else
    case "$(uname -m)" in
      arm64) BUN_ASSET="bun-darwin-aarch64" ;;
      x86_64) BUN_ASSET="bun-darwin-x64" ;;
      *) echo "ERROR: unsupported macOS arch $(uname -m)" >&2; exit 1 ;;
    esac
    BUN_DIR="$BUILD/bun"
    BUN="$BUN_DIR/$BUN_ASSET/bun"
    if [ ! -x "$BUN" ]; then
      echo "[opencode] downloading Bun"
      rm -rf "$BUN_DIR"; mkdir -p "$BUN_DIR"
      curl -fL --retry 3 -o "$BUILD/bun.zip" \
        "https://github.com/oven-sh/bun/releases/download/bun-v1.4.5/$BUN_ASSET.zip"
      unzip -q "$BUILD/bun.zip" -d "$BUN_DIR"
    fi
  fi
fi

# --- OpenCode source, pinned ------------------------------------------------
if [ ! -d "$SRC/.git" ]; then
  echo "[opencode] fetching pinned source"
  rm -rf "$SRC"
  mkdir -p "$SRC"
  git -C "$SRC" init -q
  git -C "$SRC" remote add origin https://github.com/anomalyco/opencode.git
fi
if [ "$(git -C "$SRC" rev-parse HEAD 2>/dev/null || true)" != "$PIN" ]; then
  git -C "$SRC" fetch --depth 1 origin "$PIN"
  git -C "$SRC" checkout -q --detach FETCH_HEAD
fi

echo "[opencode] installing build dependencies (this is the slow step)"
(cd "$SRC" && "$BUN" install --frozen-lockfile --os='*' --cpu='*')

echo "[opencode] building Node bundle for darwin-arm64"
(cd "$SRC" && OPENCODE_TARGET=darwin-arm64 "$BUN" run --cwd packages/cli build:node --bundle-only)

DIST="$SRC/packages/cli/dist-node"
test -f "$DIST/opencode.mjs"

# Make the native optional-dependency paths overridable so the IPA can point at
# iOS-safe JS shims instead of macOS .node/.dylib files.
python3 - "$DIST/opencode.mjs" "$OUT/opencode.mjs" <<'PY'
import re, sys
src, dst = sys.argv[1], sys.argv[2]
s = open(src, encoding="utf-8").read()
s = re.sub(r'process\.env\.OPENCODE_NODE_PTY_PATH=(__ocPath\.join\(__ocAssetRoot,"[^"]+"\))',
           r'process.env.OPENCODE_NODE_PTY_PATH??=\1', s)
s = re.sub(r'process\.env\.OPENCODE_PARCEL_WATCHER_PATH=(__ocPath\.join\(__ocAssetRoot,"[^"]+"\))',
           r'process.env.OPENCODE_PARCEL_WATCHER_PATH??=\1', s)
open(dst, "w", encoding="utf-8").write(s)
PY

rm -rf "$OUT/assets"
cp -R "$DIST/assets" "$OUT/assets"
# Drop Mach-O native code that cannot be loaded by an iOS app. The server path
# only needs the JS + WASM assets.
find "$OUT/assets" -type f \( -name '*.dylib' -o -name '*.node' -o -name 'spawn-helper' -o -name 'opencode-pty' \) -delete

cp "$ROOT/ios/node/loader.mjs" "$OUT/loader.mjs"
cp "$ROOT/ios/node/sea-shim.mjs" "$OUT/sea-shim.mjs"
cp "$ROOT/ios/node/pty-shim.cjs" "$OUT/pty-shim.cjs"
cp "$ROOT/ios/node/watcher-shim.cjs" "$OUT/watcher-shim.cjs"
printf '%s\n' "$PIN" > "$OUT/version.txt"
echo "[opencode] bundle ready: $(du -sh "$OUT" | awk '{print $1}')"