#!/usr/bin/env bash
# Build + run the native library-OS harness (Linux aarch64) to verify the KimiOS
# GUI renders without QEMU. Produces lock.ppm / home.ppm for pixel inspection.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
USER_DIR="$(cd "$HERE/../../user" && pwd)"
OUT="${1:-$HERE/out}"
mkdir -p "$OUT"

echo "[1/3] building kimi-gui staticlib for aarch64-unknown-linux-gnu"
cargo build --release -p kimi-gui --lib --target aarch64-unknown-linux-gnu \
  --manifest-path "$USER_DIR/Cargo.toml"

LIB="$USER_DIR/target/aarch64-unknown-linux-gnu/release/libkimi_gui.a"
echo "[2/3] linking C harness against $LIB"
cc -O2 -o "$OUT/kimios-host" "$HERE/harness.c" "$LIB" -lpthread -lm

echo "[3/3] running native GUI"
"$OUT/kimios-host" "$OUT"
echo "PPM frames in $OUT"
