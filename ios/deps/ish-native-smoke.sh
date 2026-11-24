#!/usr/bin/env bash
# Runtime smoke for iSH: build the `ish` CLI NATIVELY (must run on an Apple
# Silicon Mac or an arm64 Linux host — not a cross build, because the CLI is
# only defined for native builds), then boot an Alpine aarch64 rootfs and run a
# few commands inside the emulated guest. Proves the emulator + rootfs execute.
#
#   ish-native-smoke.sh [WORKDIR]
set -euo pipefail

ISH_COMMIT="c5c264da19148512167aa493d7bd4e50aae1b74e"
ISH_REPO="https://github.com/OpenMinis/ish-arm64"
ALPINE_URL="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/aarch64/alpine-minirootfs-3.21.0-aarch64.tar.gz"

WORK="${1:-$(pwd)/ios/deps/ish-native}"
mkdir -p "$WORK"

if [ ! -d "$WORK/src/.git" ]; then
    echo "[smoke] cloning iSH @ $ISH_COMMIT"
    git clone --filter=blob:none "$ISH_REPO" "$WORK/src"
    git -C "$WORK/src" checkout "$ISH_COMMIT"
fi
cd "$WORK/src"
git submodule update --init --recursive --depth 1

case "$(uname -m)" in
    arm64|aarch64) ;;
    *) echo "ERROR: need an arm64 host for the arm64 guest (got $(uname -m))"; exit 1 ;;
esac

if [ ! -f build/build.ninja ]; then
    echo "[smoke] meson setup (native)"
    meson setup build \
        --buildtype=release -Db_ndebug=true \
        -Ddefault_library=static \
        -Dlog_handler=dprintf \
        -Dkernel=ish -Dengine=asbestos -Dguest_arch=arm64 -Djit=false
fi

echo "[smoke] building ish CLI"
ninja -C build ish

VDSO=build/vdso/arm64/libvdso.so.elf
python3 - "$VDSO" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
assert len(d) >= 64 and d[:7] == b'\x7fELF\x02\x01\x01', "VDSO not ELF64"
etype, machine, _ = struct.unpack_from('<HHI', d, 16)
assert (etype, machine) == (3, 183), f"VDSO wrong etype/machine {etype}/{machine}"
print("VDSO OK")
PY

mkdir -p "$WORK/alpine"
if [ ! -f "$WORK/alpine/etc/os-release" ]; then
    echo "[smoke] fetching Alpine rootfs"
    curl -fL --retry 3 -o "$WORK/alpine.tgz" "$ALPINE_URL"
    tar -xzf "$WORK/alpine.tgz" -C "$WORK/alpine"
fi

echo "[smoke] running guest"
set +e
OUT=$(timeout 180 ./build/ish -r "$WORK/alpine" /bin/sh -c \
    'echo HELLO_ISH; cat /etc/os-release | head -1; echo "cwd=$(pwd)"; ls / | head -5; exit 7' 2>&1)
RC=$?
set -e
printf '%s\n' "$OUT"
echo "[smoke] guest exit status: $RC (expected 7)"

echo "$OUT" | grep -q 'HELLO_ISH' || { echo "FAIL: guest did not run"; exit 1; }
echo "[smoke] PASS: real Linux guest executed on $(uname -s) $(uname -m)"