#!/usr/bin/env bash
# Build + run the runtime-3D host harness: loads a GLB and renders one frame
# through the real k3d pipeline natively (no QEMU, no device).
#
#   ./build_pet3d.sh [model.glb] [out-dir]
#
# Defaults to assets/models/testchar.glb and ios/host-harness/out.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
USER_DIR="$ROOT/user"
MODEL="${1:-$ROOT/assets/models/testchar.glb}"
OUT="${2:-$HERE/out}"
mkdir -p "$OUT"

echo "[1/3] building kimi-gui staticlib for the host target"
# The userland crate pins an aarch64 target in its cargo config, so the host
# target must be requested explicitly.
cargo build --release -p kimi-gui --lib --target x86_64-unknown-linux-gnu \
  --manifest-path "$USER_DIR/Cargo.toml"

LIB="$USER_DIR/target/x86_64-unknown-linux-gnu/release/libkimi_gui.a"
echo "[2/3] linking C harness against $LIB"
cc -O2 -o "$OUT/kimios-pet3d" "$HERE/harness_pet3d.c" "$LIB" -lpthread -lm

echo "[3/3] rendering $MODEL"
"$OUT/kimios-pet3d" "$MODEL"