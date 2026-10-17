#!/bin/sh
# Build a local development bundle using Apple's native tools.
set -eu
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
    echo "Build this bundle on macOS with Apple's Command Line Tools installed." >&2
    exit 1
fi
cargo build --release --locked
bundle=target/macos/Heya.app
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp target/release/heya "$bundle/Contents/MacOS/heya"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>org.heya.Heya</string>
  <key>CFBundleName</key><string>Heya</string>
  <key>CFBundleDisplayName</key><string>Heya</string>
  <key>CFBundleExecutable</key><string>heya</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
plutil -lint "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
echo "Built $bundle"
