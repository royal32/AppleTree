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
ICONSET_DIR="$PACKAGE_DIR/$APP_NAME.iconset"
ICON_SOURCE="$ROOT_DIR/launcher_icon.png"
ICON_FILE="$RESOURCES_DIR/$APP_NAME.icns"
SWIFT_MODULE_CACHE="$ROOT_DIR/target/swift-module-cache"

if [[ ! -f "$ICON_SOURCE" ]]; then
  echo "Missing icon source: $ICON_SOURCE" >&2
  exit 1
fi

for command in cargo swift python3; do
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

rm -rf "$APP_DIR" "$ICONSET_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$ICONSET_DIR" "$SWIFT_MODULE_CACHE"

cp "$BUILD_DIR/$BINARY_NAME" "$MACOS_DIR/$BINARY_NAME"
chmod 755 "$MACOS_DIR/$BINARY_NAME"

CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" swift -module-cache-path "$SWIFT_MODULE_CACHE" - "$ICON_SOURCE" "$ICONSET_DIR" <<'SWIFT'
import AppKit
import Foundation

let source = URL(fileURLWithPath: CommandLine.arguments[1])
let outputDir = URL(fileURLWithPath: CommandLine.arguments[2])

guard let sourceImage = NSImage(contentsOf: source) else {
  fatalError("could not load icon source")
}

func writeIcon(_ name: String, _ pixels: Int) throws {
  let size = NSSize(width: pixels, height: pixels)
  let image = NSImage(size: size)

  image.lockFocus()
  NSGraphicsContext.current?.imageInterpolation = .high
  sourceImage.draw(
    in: NSRect(origin: .zero, size: size),
    from: .zero,
    operation: .copy,
    fraction: 1.0
  )
  image.unlockFocus()

  guard let tiff = image.tiffRepresentation,
        let rep = NSBitmapImageRep(data: tiff),
        let data = rep.representation(using: .png, properties: [:]) else {
    fatalError("could not render icon")
  }

  try data.write(to: outputDir.appendingPathComponent(name))
}

try writeIcon("icon_16x16.png", 16)
try writeIcon("icon_16x16@2x.png", 32)
try writeIcon("icon_32x32.png", 32)
try writeIcon("icon_32x32@2x.png", 64)
try writeIcon("icon_128x128.png", 128)
try writeIcon("icon_128x128@2x.png", 256)
try writeIcon("icon_256x256.png", 256)
try writeIcon("icon_256x256@2x.png", 512)
try writeIcon("icon_512x512.png", 512)
try writeIcon("icon_512x512@2x.png", 1024)
SWIFT

python3 - "$ICONSET_DIR" "$ICON_FILE" <<'PY'
from pathlib import Path
import struct
import sys

iconset = Path(sys.argv[1])
output = Path(sys.argv[2])
entries = [
    ("icp4", "icon_16x16.png"),
    ("icp5", "icon_32x32.png"),
    ("icp6", "icon_32x32@2x.png"),
    ("ic07", "icon_128x128.png"),
    ("ic08", "icon_256x256.png"),
    ("ic09", "icon_512x512.png"),
    ("ic10", "icon_512x512@2x.png"),
]

chunks = []
for icon_type, filename in entries:
    data = (iconset / filename).read_bytes()
    chunks.append(
        icon_type.encode("ascii") + struct.pack(">I", len(data) + 8) + data
    )

output.write_bytes(
    b"icns" + struct.pack(">I", 8 + sum(len(chunk) for chunk in chunks)) + b"".join(chunks)
)
PY

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
rm -rf "$ICONSET_DIR"

if [[ "$CODESIGN" != "0" ]]; then
  codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP_DIR"
fi

echo "Packaged $APP_DIR"
