#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${1:-"$SCRIPT_DIR/dist"}"
BIN="$SCRIPT_DIR/target/release/ds5dongle-ota-updater"
APP_NAME="DS5Dongle OTA Updater.app"
APP="$OUT_DIR/$APP_NAME"
ZIP="$OUT_DIR/ds5dongle-ota-updater-macOS.app.zip"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS packaging must run on Darwin." >&2
  exit 1
fi

if [[ ! -x "$BIN" ]]; then
  echo "Missing release binary: $BIN" >&2
  echo "Run: cargo build --release --manifest-path tools/ota-updater/Cargo.toml" >&2
  exit 1
fi

rm -rf "$APP" "$ZIP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/ds5dongle-ota-updater"
chmod +x "$APP/Contents/MacOS/ds5dongle-ota-updater"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleExecutable</key>
  <string>ds5dongle-ota-updater</string>
  <key>CFBundleIdentifier</key>
  <string>top.mainlxl.ds5dongle-ota-updater</string>
  <key>CFBundleName</key>
  <string>DS5Dongle OTA Updater</string>
  <key>CFBundleDisplayName</key>
  <string>DS5Dongle OTA Updater</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.15</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$APP/Contents/PkgInfo"

plutil -lint "$APP/Contents/Info.plist"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

echo "$APP"
echo "$ZIP"
