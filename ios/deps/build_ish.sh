#!/usr/bin/env bash
# Build iSH-ARM64 (OpenMinis/ish-arm64) as static libs for the iOS device,
# plus the `fakefsify` tool used to convert an Alpine rootfs to iSH fakefs.
#
# This is the Linux userland that lets KimiOS *execute* real Linux binaries on
# iOS (no fork/exec of Linux ELF, no JIT): iSH emulates an AArch64 Linux guest
# with a threaded-code interpreter and translates syscalls to Darwin.
#
# Output: ios/deps/ish/build-ios/{libish.a,libish_emu.a,libfakefs.a,tools/fakefsify}
# Headers stay in the iSH tree; the Xcode project adds ios/deps/ish and
# ios/deps/ish/build-ios to HEADER_SEARCH_PATHS.
#
# Requires macOS + Xcode + meson + ninja (+ llvm for the guest VDSO).
set -euo pipefail

ISH_COMMIT="c5c264da19148512167aa493d7bd4e50aae1b74e"
ISH_REPO="https://github.com/OpenMinis/ish-arm64"
IOS_DEPLOYMENT_TARGET="${IOS_DEPLOYMENT_TARGET:-15.0}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISH_DIR="$SCRIPT_DIR/ish"
BUILD_DIR="$ISH_DIR/build-ios"

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

SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
mkdir -p "$BUILD_DIR"
CROSS="$BUILD_DIR/ios-cross.txt"
cat > "$CROSS" <<EOF
[binaries]
c = ['clang', '-arch', 'arm64', '-isysroot', '$SDK', '-miphoneos-version-min=$IOS_DEPLOYMENT_TARGET']
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

if [ ! -f "$BUILD_DIR/build.ninja" ]; then
    echo "[ish] meson setup"
    meson setup "$BUILD_DIR" \
        --cross-file "$CROSS" \
        --buildtype=release \
        -Db_ndebug=true \
        -Dlog="" \
        -Dlog_handler=nslog \
        -Dkernel=ish \
        -Dengine=asbestos \
        -Dguest_arch=arm64
fi

echo "[ish] building libish/libish_emu/libfakefs (iOS device)"
ninja -C "$BUILD_DIR" libish.a libish_emu.a libfakefs.a

# `fakefsify` is a build-time host tool (uses libarchive, runs on the Mac to
# convert the Alpine tarball into fakefs), so it must be a native build.
HOST_BUILD="$ISH_DIR/build-host"
if [ ! -f "$HOST_BUILD/build.ninja" ]; then
    echo "[ish] native (host) build for fakefsify"
    meson setup "$HOST_BUILD" --buildtype=release
fi
ninja -C "$HOST_BUILD" tools/fakefsify

echo "[ish] done:"
ls -lh "$BUILD_DIR"/libish.a "$BUILD_DIR"/libish_emu.a "$BUILD_DIR"/libfakefs.a "$HOST_BUILD"/tools/fakefsify