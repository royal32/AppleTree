#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICON_PNG_SOURCE="${ICON_PNG_SOURCE:-$ROOT_DIR/AppIcon.png}"
ICON_PNG_OUTPUT="${ICON_PNG_OUTPUT:-$ROOT_DIR/AppIcon.png}"
ICON_ICNS_OUTPUT="${ICON_ICNS_OUTPUT:-$ROOT_DIR/AppIcon.icns}"
ICON_CONTENT_SIZE="${ICON_CONTENT_SIZE:-896}"
WORK_DIR="${WORK_DIR:-$ROOT_DIR/target/macos-icon}"
ICONSET_DIR="$WORK_DIR/AppIcon.iconset"
ADJUSTED_PNG="$WORK_DIR/AppIcon.png"
SWIFT_MODULE_CACHE="$ROOT_DIR/target/swift-module-cache"

if [[ ! -f "$ICON_PNG_SOURCE" ]]; then
  echo "Missing icon PNG source: $ICON_PNG_SOURCE" >&2
  exit 1
fi

for command in swift sips python3; do
  if ! command -v "$command" >/dev/null; then
    echo "Missing required command: $command" >&2
    exit 1
  fi
done

rm -rf "$ICONSET_DIR"
mkdir -p "$ICONSET_DIR" "$SWIFT_MODULE_CACHE"

CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" swift -module-cache-path "$SWIFT_MODULE_CACHE" - "$ICON_PNG_SOURCE" "$ADJUSTED_PNG" "$ICON_CONTENT_SIZE" <<'SWIFT'
import AppKit
import Foundation

let source = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
let contentSize = CGFloat(Double(CommandLine.arguments[3])!)
let canvasSize = CGFloat(1024)

guard let sourceImage = NSImage(contentsOf: source) else {
  fatalError("could not load source image")
}

let image = NSImage(size: NSSize(width: canvasSize, height: canvasSize))
image.lockFocus()
NSColor.clear.setFill()
NSRect(x: 0, y: 0, width: canvasSize, height: canvasSize).fill()
NSGraphicsContext.current?.imageInterpolation = .high

let inset = (canvasSize - contentSize) / 2
sourceImage.draw(
  in: NSRect(x: inset, y: inset, width: contentSize, height: contentSize),
  from: NSRect(x: 0, y: 0, width: sourceImage.size.width, height: sourceImage.size.height),
  operation: .copy,
  fraction: 1.0
)
image.unlockFocus()

guard let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let data = rep.representation(using: .png, properties: [:]) else {
  fatalError("could not write PNG")
}

try data.write(to: output)
SWIFT

sips --resampleHeightWidth 16 16 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_16x16.png" >/dev/null
sips --resampleHeightWidth 32 32 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_16x16@2x.png" >/dev/null
sips --resampleHeightWidth 32 32 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_32x32.png" >/dev/null
sips --resampleHeightWidth 64 64 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_32x32@2x.png" >/dev/null
sips --resampleHeightWidth 48 48 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_48x48.png" >/dev/null
sips --resampleHeightWidth 128 128 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_128x128.png" >/dev/null
sips --resampleHeightWidth 256 256 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
sips --resampleHeightWidth 256 256 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_256x256.png" >/dev/null
sips --resampleHeightWidth 512 512 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_256x256@2x.png" >/dev/null
sips --resampleHeightWidth 512 512 "$ADJUSTED_PNG" --out "$ICONSET_DIR/icon_512x512.png" >/dev/null
cp "$ADJUSTED_PNG" "$ICONSET_DIR/icon_512x512@2x.png"

python3 - "$ICONSET_DIR" "$ICON_ICNS_OUTPUT" <<'PY'
from pathlib import Path
import struct
import sys

iconset = Path(sys.argv[1])
output = Path(sys.argv[2])
entries = [
    ("icp4", "icon_16x16.png"),
    ("icp5", "icon_32x32.png"),
    ("icp6", "icon_48x48.png"),
    ("ic07", "icon_128x128.png"),
    ("ic08", "icon_256x256.png"),
    ("ic09", "icon_512x512.png"),
    ("ic10", "icon_512x512@2x.png"),
    ("ic11", "icon_16x16@2x.png"),
    ("ic12", "icon_32x32@2x.png"),
    ("ic13", "icon_128x128@2x.png"),
    ("ic14", "icon_256x256@2x.png"),
]

chunks = []
for icon_type, filename in entries:
    data = (iconset / filename).read_bytes()
    chunks.append(icon_type.encode("ascii") + struct.pack(">I", len(data) + 8) + data)

output.write_bytes(
    b"icns" + struct.pack(">I", 8 + sum(len(chunk) for chunk in chunks)) + b"".join(chunks)
)
PY

cp "$ADJUSTED_PNG" "$ICON_PNG_OUTPUT"

echo "Prepared $ICON_PNG_OUTPUT"
echo "Prepared $ICON_ICNS_OUTPUT"
