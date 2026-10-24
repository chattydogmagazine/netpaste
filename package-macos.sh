#!/bin/bash
# Build NetPaste.app. With both Rust targets installed this produces one
# universal binary that runs on Intel iMacs and Apple-silicon Macs alike.
set -euo pipefail
cd "$(dirname "$0")"

APP="NetPaste.app"
VERSION=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
BIN="target/release/NetPaste"

have() { rustup target list --installed 2>/dev/null | grep -qx "$1"; }

if have x86_64-apple-darwin && have aarch64-apple-darwin; then
  echo "Building a universal binary (Intel + Apple silicon)…"
  cargo build --release --target x86_64-apple-darwin
  cargo build --release --target aarch64-apple-darwin
  BIN="target/universal-NetPaste"
  lipo -create -output "$BIN" \
    target/x86_64-apple-darwin/release/NetPaste \
    target/aarch64-apple-darwin/release/NetPaste
else
  echo "Building for this Mac only."
  echo "For a universal build first run:"
  echo "  rustup target add x86_64-apple-darwin aarch64-apple-darwin"
  cargo build --release
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp branding/icons/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp "$BIN" "$APP/Contents/MacOS/NetPaste"
chmod +x "$APP/Contents/MacOS/NetPaste"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>NetPaste</string>
  <key>CFBundleDisplayName</key><string>NetPaste</string>
  <key>CFBundleExecutable</key><string>NetPaste</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>local.netpaste.app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>__VERSION__</string>
  <key>CFBundleVersion</key><string>__VERSION__</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>NetPaste finds your other machines on this network so they can share text and files.</string>
</dict>
</plist>
PLIST

# stamp the real version into the bundle
sed -i '' "s/__VERSION__/$VERSION/g" "$APP/Contents/Info.plist"

echo
echo "Built $APP - double-click it, or drag it to /Applications."
file "$APP/Contents/MacOS/NetPaste"
