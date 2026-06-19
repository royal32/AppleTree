#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${APP_NAME:-AppleTree}"
BUNDLE_ID="${BUNDLE_ID:-com.sethphillips.appletree}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
CODESIGN="${CODESIGN:-1}"
VERSION="$(awk -F\" '/^version = / { print $2; exit }' "$ROOT_DIR/Cargo.toml")"
BINARY_NAME="$(awk -F\" '/^name = / { print $2; exit }' "$ROOT_DIR/Cargo.toml")"
BUILD_DIR="$ROOT_DIR/target/release"
PACKAGE_DIR="$BUILD_DIR/macos"
APP_DIR="$PACKAGE_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
ICON_SOURCE="${ICON_SOURCE:-$ROOT_DIR/AppIcon.icns}"
ICON_FILE="$RESOURCES_DIR/$APP_NAME.icns"

if [[ ! -f "$ICON_SOURCE" ]]; then
  echo "Missing app icon: $ICON_SOURCE" >&2
  exit 1
fi

for command in cargo; do
  if ! command -v "$command" >/dev/null; then
    echo "Missing required command: $command" >&2
    exit 1
  fi
done

if [[ "$CODESIGN" != "0" ]] && ! command -v codesign >/dev/null; then
  echo "Missing required command: codesign" >&2
  exit 1
fi

cargo build --release

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$BUILD_DIR/$BINARY_NAME" "$MACOS_DIR/$BINARY_NAME"
chmod 755 "$MACOS_DIR/$BINARY_NAME"
cp "$ICON_SOURCE" "$ICON_FILE"

cat >"$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleExecutable</key>
  <string>$BINARY_NAME</string>
  <key>CFBundleIconFile</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright 2026 Seth Phillips. Licensed under GPL-3.0.</string>
</dict>
</plist>
PLIST

printf "APPL????" >"$CONTENTS_DIR/PkgInfo"

if [[ "$CODESIGN" != "0" ]]; then
  codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP_DIR"
fi

echo "Packaged $APP_DIR"
