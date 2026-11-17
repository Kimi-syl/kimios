#!/usr/bin/env bash
# Build the KimiOS library-OS iOS app and package an (unsigned) .ipa.
#
# The pure-Rust userland (libkimi + kimi-gui) is compiled as a no_std staticlib
# for aarch64-apple-ios and linked into a Swift/UIKit/Metal shell that implements
# the host syscall dispatcher. No QEMU, no EL1/MMU — native ARM64 on the iPad.
#
# Requires macOS + Xcode + Rust. The .ipa is unsigned; install via Sideloadly /
# AltStore / StikDebug (re-signs with your Apple ID), or sign with your cert.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
USER_DIR="$ROOT/user"
APP_DIR="$ROOT/ios/app"
BUILD_DIR="$ROOT/ios/build"

echo "[1/5] building Rust staticlib for aarch64-apple-ios"
rustup target add aarch64-apple-ios
cargo build --release -p kimi-gui --lib --target aarch64-apple-ios \
  --manifest-path "$USER_DIR/Cargo.toml"
cp "$USER_DIR/target/aarch64-apple-ios/release/libkimi_gui.a" "$APP_DIR/libkimi_gui.a"

echo "[2/5] generating Xcode project (xcodegen)"
command -v xcodegen >/dev/null 2>&1 || brew install xcodegen
(cd "$APP_DIR" && xcodegen generate)

echo "[3/5] xcodebuild (unsigned, iphoneos)"
xcodebuild \
  -project "$APP_DIR/KimiOS.xcodeproj" \
  -scheme KimiOS \
  -configuration Release \
  -sdk iphoneos \
  -derivedDataPath "$BUILD_DIR" \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" \
  build

echo "[4/5] packaging .ipa"
APP="$BUILD_DIR/Build/Products/Release-iphoneos/KimiOS.app"
test -d "$APP" || { echo "ERROR: $APP not found"; exit 1; }
rm -rf "$BUILD_DIR/Payload"
mkdir -p "$BUILD_DIR/Payload"
cp -R "$APP" "$BUILD_DIR/Payload/"
(cd "$BUILD_DIR" && zip -qry KimiOS.ipa Payload)

echo "[5/5] done: $BUILD_DIR/KimiOS.ipa"
ls -la "$BUILD_DIR/KimiOS.ipa"
