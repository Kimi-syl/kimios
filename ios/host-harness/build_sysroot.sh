#!/usr/bin/env bash
# Build + run the apkg sysroot harness: compiles the unmodified libkimi userland
# (via the shelltest staticlib) for the host and links harness_apkg.c, then runs
# `apkg update` + `apkg install python3` + `apkg install nodejs` against the live
# Alpine CDN, extracting into a host sysroot.
#
# /disk maps to $KIMI_DISK_ROOT (default: a fresh mkdtemp dir). Point it at a
# persistent directory to inspect the extracted sysroot afterwards, e.g.
#   KIMI_DISK_ROOT=/tmp/opencode/sysroot ./build_sysroot.sh
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/opencode/target-sysroot}"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/out"
mkdir -p "$OUT"

echo "[1/3] building shelltest staticlib (libkimi userland) for aarch64-unknown-linux-gnu"
cargo build --release --target aarch64-unknown-linux-gnu \
  --manifest-path "$HERE/shelltest/Cargo.toml"

LIB="$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release/libshelltest.a"
echo "[2/3] linking harness_apkg.c against $LIB"
cc -O2 -o "$OUT/apkg-host" "$HERE/harness_apkg.c" "$LIB" -lpthread -lm

echo "[3/3] running apkg sysroot test"
"$OUT/apkg-host"
echo "sysroot under: ${KIMI_DISK_ROOT:-<mkdtemp dir printed above>}/alpine/root"
