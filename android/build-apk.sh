#!/usr/bin/env bash
# Build the KimiOS library-OS Android APK (native ARM64, no QEMU).
#
# Pipeline: Rust no_std staticlib (aarch64-linux-android) -> C JNI dispatcher
# linked into libkimios.so -> javac + d8 (classes.dex) -> aapt2 (resources +
# manifest) -> package dex + lib -> zipalign -> apksigner (debug key).
#
# Works on the phone build host (/opt/tusr toolchain) and on CI (Android NDK).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_DIR="$ROOT/android"
USER_DIR="$ROOT/user"
OUT="$ANDROID_DIR/build"
mkdir -p "$OUT"

SDK="${ANDROID_SDK:-/opt/android-sdk}"
BT="$SDK/build-tools/34.0.0"
ANDROID_JAR="$SDK/platforms/android-34/android.jar"
NDK="${ANDROID_NDK:-$SDK/ndk/26.1.10909125}"
export JAVA_HOME="${JAVA_HOME:-/opt/jdk17}"
export PATH="$JAVA_HOME/bin:$HOME/.cargo/bin:$PATH"
KS="${KIMIOS_KEYSTORE:-$HOME/.android/debug.keystore}"
if [ ! -f "$KS" ]; then
  KS="$OUT/debug.keystore"
  "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$KS" -storepass android \
    -keypass android -alias androiddebugkey -dname "CN=KimiOS Debug,O=KimiOS,C=US" \
    -keyalg RSA -keysize 2048 -validity 10000 >/dev/null 2>&1
fi

# arm-native aapt2/zipalign on the phone host; SDK copies on CI.
AAPT2="$BT/aapt2"; ZIPALIGN="$BT/zipalign"
[ -x /opt/aapt2arm/build-tools/aapt2 ] && AAPT2=/opt/aapt2arm/build-tools/aapt2
[ -x /opt/aapt2arm/build-tools/zipalign ] && ZIPALIGN=/opt/aapt2arm/build-tools/zipalign

echo "[1/7] Rust staticlib (aarch64-linux-android)"
rustup target add aarch64-linux-android >/dev/null 2>&1 || true
cargo build --release -p kimi-gui --lib --target aarch64-linux-android \
  --manifest-path "$USER_DIR/Cargo.toml"
RUSTLIB="$USER_DIR/target/aarch64-linux-android/release/libkimi_gui.a"

echo "[2/7] native libkimios.so (C dispatcher + Rust staticlib)"
if [ -x /opt/tusr/bin/clang ]; then
  unset TERMUX_EXEC__PROC_SELF_EXE
  CC=/opt/tusr/bin/clang
  CFLAGS="--target=aarch64-linux-android26 --sysroot=/opt/tusr"
  SYSLIB=/opt/tusr/lib
else
  PREBUILT="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
  CC="$PREBUILT/bin/aarch64-linux-android26-clang"
  CFLAGS=""
  SYSLIB="$PREBUILT/sysroot/usr/lib/aarch64-linux-android/26"
fi
"$CC" $CFLAGS -shared -fPIC -O2 -o "$OUT/libkimios.so" \
  "$ANDROID_DIR/jni/kimios_jni.c" "$ANDROID_DIR/jni/kimi_elf.c" "$RUSTLIB" \
  -L"$SYSLIB" -landroid -llog

echo "[3/7] javac + d8"
rm -rf "$OUT/classes" "$OUT/dex"; mkdir -p "$OUT/classes" "$OUT/dex"
"$JAVA_HOME/bin/javac" -source 8 -target 8 -bootclasspath "$ANDROID_JAR" \
  -d "$OUT/classes" $(find "$ANDROID_DIR/java" -name '*.java')
"$BT/d8" --release --lib "$ANDROID_JAR" --output "$OUT/dex" \
  $(find "$OUT/classes" -name '*.class')

echo "[4/7] aapt2 compile + link"
"$AAPT2" compile --dir "$ANDROID_DIR/res" -o "$OUT/res.zip"
ASSETS_ARG=""
[ -d "$ANDROID_DIR/assets" ] && ASSETS_ARG="-A $ANDROID_DIR/assets"
"$AAPT2" link -o "$OUT/base.apk" -I "$ANDROID_JAR" \
  --manifest "$ANDROID_DIR/AndroidManifest.xml" "$OUT/res.zip" --auto-add-overlay $ASSETS_ARG

echo "[5/7] package classes.dex + native lib"
cp "$OUT/base.apk" "$OUT/unsigned.apk"
python3 - "$OUT/unsigned.apk" "$OUT/dex/classes.dex" "$OUT/libkimios.so" <<'PY'
import sys, zipfile
apk, dex, so = sys.argv[1:4]
with zipfile.ZipFile(apk, 'a', compression=zipfile.ZIP_DEFLATED) as z:
    z.write(dex, 'classes.dex')
    z.write(so, 'lib/arm64-v8a/libkimios.so')
PY

echo "[6/7] zipalign"
"$ZIPALIGN" -f 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"

echo "[7/7] apksigner (debug key)"
"$BT/apksigner" sign --ks "$KS" --ks-pass pass:android --key-pass pass:android \
  --ks-key-alias androiddebugkey --out "$OUT/KimiOS-libos.apk" "$OUT/aligned.apk"
"$BT/apksigner" verify "$OUT/KimiOS-libos.apk"

echo "DONE: $OUT/KimiOS-libos.apk"
ls -la "$OUT/KimiOS-libos.apk"
