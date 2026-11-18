#!/usr/bin/env bash
# Build the host ELF-loader validation harness (aarch64 Linux). Compiles the
# device loader android/jni/kimi_elf.c with -DKIMI_ELF_NO_JNI (JNI section
# excluded) + a logcat->stderr stub, linked with test_main.c.
#
#   ./build.sh
#   SR=/tmp/opencode/sysroot/alpine/root           # from build_sysroot.sh
#   ./out/elftest $SR/hello_dyn $SR                 # dynamic musl hello
#   ./out/elftest $SR/usr/bin/python3 $SR 'print(12345)'
#   ./out/elftest $SR/usr/bin/node $SR 'console.log(6*7)' -e
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
mkdir -p "$HERE/out"
cc -O2 -DKIMI_ELF_NO_JNI -I"$HERE" -o "$HERE/out/elftest" \
  "$HERE/test_main.c" "$ROOT/android/jni/kimi_elf.c"
echo "built: $HERE/out/elftest"
