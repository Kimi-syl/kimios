#!/usr/bin/env bash
# Build iSH-ARM64 (OpenMinis/ish-arm64) as static libs for iOS, plus (optionally)
# the `ish` CLI used by the runtime smoke test. Also builds the native
# `fakefsify` host tool for converting an Alpine rootfs to iSH fakefs.
#
#   build_ish.sh [iphoneos|iphonesimulator] [--cli]
#
# Default: iphoneos static libs. With --cli, also builds the `ish` executable
# (used by the simulator smoke to actually run the guest in CI).
#
# iSH emulates an AArch64 Linux guest with a threaded-code interpreter and
# translates syscalls to Darwin, so KimiOS can execute real Linux binaries on
# iOS without fork/exec or JIT.
# Requires macOS + Xcode + meson + ninja (+ llvm/lld for the guest VDSO).
set -euo pipefail

ISH_COMMIT="c5c264da19148512167aa493d7bd4e50aae1b74e"
ISH_REPO="https://github.com/OpenMinis/ish-arm64"
IOS_DEPLOYMENT_TARGET="${IOS_DEPLOYMENT_TARGET:-15.0}"

PLATFORM="iphoneos"
BUILD_CLI=0
for arg in "$@"; do
    case "$arg" in
        iphoneos|iphonesimulator) PLATFORM="$arg" ;;
        --cli) BUILD_CLI=1 ;;
        *) echo "unknown arg: $arg"; exit 1 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISH_DIR="$SCRIPT_DIR/ish"
if [ "$PLATFORM" = "iphoneos" ]; then
    BUILD_DIR="$ISH_DIR/build-ios"
else
    BUILD_DIR="$ISH_DIR/build-ios-sim"
fi

# ---- source (pinned) ----
if [ ! -d "$ISH_DIR/.git" ]; then
    echo "[ish] cloning $ISH_REPO @ $ISH_COMMIT"
    rm -rf "$ISH_DIR"
    git clone --filter=blob:none "$ISH_REPO" "$ISH_DIR"
    git -C "$ISH_DIR" checkout "$ISH_COMMIT"
fi
echo "[ish] initialising submodules"
git -C "$ISH_DIR" submodule update --init --recursive --depth 1
cd "$ISH_DIR"

command -v meson >/dev/null || { echo "ERROR: pip3 install meson"; exit 1; }
command -v ninja >/dev/null || { echo "ERROR: brew install ninja"; exit 1; }

# ---- cross file ----
SDK="$(xcrun --sdk "$PLATFORM" --show-sdk-path)"
if [ "$PLATFORM" = "iphoneos" ]; then
    MIN_FLAG="-miphoneos-version-min=$IOS_DEPLOYMENT_TARGET"
    TARGET_TRIPLE="arm64-apple-ios$IOS_DEPLOYMENT_TARGET"
else
    MIN_FLAG="-mios-simulator-version-min=$IOS_DEPLOYMENT_TARGET"
    TARGET_TRIPLE="arm64-apple-ios$IOS_DEPLOYMENT_TARGET-simulator"
fi
mkdir -p "$BUILD_DIR"
CROSS="$BUILD_DIR/ios-cross.txt"
cat > "$CROSS" <<EOF
[binaries]
c = ['clang', '-target', '$TARGET_TRIPLE', '-isysroot', '$SDK', '$MIN_FLAG']
ar = 'ar'
strip = 'strip'
pkg-config = 'false'

[host_machine]
system = 'darwin'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'

[properties]
needs_exe_wrapper = true
sys_root = '$SDK'
EOF

# ---- configure ----
if [ ! -f "$BUILD_DIR/build.ninja" ]; then
    echo "[ish] meson setup ($PLATFORM)"
    meson setup "$BUILD_DIR" \
        --cross-file "$CROSS" \
        --buildtype=release \
        -Db_ndebug=true \
        -Ddefault_library=static \
        -Db_staticpic=true \
        -Dlog="" \
        -Dlog_handler=nslog \
        -Dkernel=ish \
        -Dengine=asbestos \
        -Dguest_arch=arm64 \
        -Djit=false
fi

echo "[ish] building libish/libish_emu/libfakefs ($PLATFORM)"
ninja -C "$BUILD_DIR" libish.a libish_emu.a libfakefs.a
if [ "$BUILD_CLI" = "1" ]; then
    echo "[ish] building ish CLI ($PLATFORM)"
    ninja -C "$BUILD_DIR" ish
fi

# ---- verify the guest VDSO is a real ELF64/AArch64 ET_DYN ----
VDSO="$BUILD_DIR/vdso/arm64/libvdso.so.elf"
python3 - "$VDSO" <<'PY'
import struct, sys
p = sys.argv[1]
d = open(p, 'rb').read()
if len(d) < 64 or d[:7] != b'\x7fELF\x02\x01\x01':
    sys.exit("VDSO is not ELF64 little-endian: " + p)
etype, machine, _ = struct.unpack_from('<HHI', d, 16)
if (etype, machine) != (3, 183):
    sys.exit(f"VDSO wrong type/machine: etype={etype} machine={machine}")
print("VDSO OK:", p)
PY

# ---- native fakefsify host tool (needs libarchive; used to make the rootfs) ----
HOST_BUILD="$ISH_DIR/build-host"
if [ ! -f "$HOST_BUILD/build.ninja" ]; then
    echo "[ish] native (host) build for fakefsify"
    meson setup "$HOST_BUILD" --buildtype=release
fi
ninja -C "$HOST_BUILD" tools/fakefsify

# ---- build receipt ----
python3 - "$BUILD_DIR" "$PLATFORM" <<'PY'
import hashlib, json, pathlib, sys
bdir = pathlib.Path(sys.argv[1]); plat = sys.argv[2]
prods = {}
for n in ("libish.a", "libish_emu.a", "libfakefs.a"):
    f = bdir / n
    if f.is_file():
        prods[n] = hashlib.sha256(f.read_bytes()).hexdigest()
out = {"platform": plat, "products": prods}
(bdir / ".ish-build.json").write_text(json.dumps(out, indent=2) + "\n")
print("receipt written:", bdir / ".ish-build.json")
PY

echo "[ish] done ($PLATFORM):"
ls -lh "$BUILD_DIR"/libish.a "$BUILD_DIR"/libish_emu.a "$BUILD_DIR"/libfakefs.a \
       "$HOST_BUILD"/tools/fakefsify 2>/dev/null