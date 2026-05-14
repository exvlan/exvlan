#!/usr/bin/env sh
set -e

ROOT_DIR="$(pwd)"
echo "Cleaning build residues in $ROOT_DIR..."
rm -rf "$ROOT_DIR/exvlan/plugin/vpnservice/android/.tauri"
rm -rf "$ROOT_DIR/exvlan/plugin/vpnservice/android/build"

rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/.gradle"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/.kotlin"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/build"
rm -f "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/proguard-tauri.pro"
rm -f "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/tauri.build.gradle.kts"
rm -f "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/tauri.properties"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/build"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/buildSrc/.gradle"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/buildSrc/.kotlin"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/buildSrc/build"
rm -f "$ROOT_DIR/packages/studio/src-tauri/gen/android/tauri.settings.gradle"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/schemas"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/src/main/jniLibs"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/src/main/java/era/exvlan/studio/generated"
rm -rf "$ROOT_DIR/packages/studio/src-tauri/gen/android/app/src/main/assets"

echo "Cleaned build residues."

rm -rf node_modules
find ./ -name "node_modules" -type d -exec rm -r {} + 2>/dev/null || true
find ./ -name "dist" -type d -exec rm -r {} + 2>/dev/null || true

cargo clean
echo "Cleaned cargo build."
echo "Cleaned build residues."
echo "Cleaned node_modules."
echo "Cleaned dist."
echo "Cleaned cargo build."