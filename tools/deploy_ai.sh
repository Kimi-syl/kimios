#!/usr/bin/env bash
# Deploy llama-server + a GGUF model to the KimiOS app sandbox for AI Studio.
# Usage: tools/deploy_ai.sh <device-ip:port> [model.gguf]
# Requires: adb, the app installed (com.kimios.libos), run-as usable (debuggable).
set -euo pipefail
ADB=${ADB:-/opt/aapt2arm/platform-tools/adb}
DEV=${1:?usage: deploy_ai.sh <ip:port> [model.gguf]}
MODEL=${2:-/root/models/LFM2.5-230M-Q4_K_M.gguf}
BIN=${LLAMA_SERVER:-/root/llama-build/llama-server}
PKG=com.kimios.libos

$ADB connect "$DEV" >/dev/null 2>&1 || true
$ADB -s "$DEV" wait-for-device

tmp=$(mktemp -d)
mkdir -p "$tmp/bin" "$tmp/models"
cp "$BIN" "$tmp/bin/llama-server"
cp "$MODEL" "$tmp/models/$(basename "$MODEL")"

echo "[1/3] pushing llama-server ($(du -h "$BIN" | cut -f1))..."
$ADB -s "$DEV" push "$tmp/bin/llama-server" /data/local/tmp/llama-server
echo "[2/3] installing into sysroot/usr/bin..."
$ADB -s "$DEV" shell "run-as $PKG sh -c 'mkdir -p files/disk/alpine/root/usr/bin files/disk/models; cat /data/local/tmp/llama-server > files/disk/alpine/root/usr/bin/llama-server; chmod 755 files/disk/alpine/root/usr/bin/llama-server'"

echo "[3/3] pushing model ($(du -h "$MODEL" | cut -f1))..."
$ADB -s "$DEV" push "$MODEL" "/data/local/tmp/$(basename "$MODEL")"
$ADB -s "$DEV" shell "run-as $PKG sh -c 'cat /data/local/tmp/$(basename "$MODEL") > files/disk/models/$(basename "$MODEL")'"
$ADB -s "$DEV" shell "rm -f /data/local/tmp/llama-server /data/local/tmp/$(basename "$MODEL")"

rm -rf "$tmp"
echo "done. AI Studio -> Start should load $(basename "$MODEL")."