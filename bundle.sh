#!/bin/sh
# Builds dist/EasySCP.app, a double-clickable macOS app.
# Drag it to /Applications (or the Dock) once it's built.
set -eu
cd "$(dirname "$0")"

cargo build --release

app=dist/EasySCP.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/easyscp "$app/Contents/MacOS/EasySCP"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>EasySCP</string>
    <key>CFBundleDisplayName</key><string>EasySCP</string>
    <key>CFBundleIdentifier</key><string>local.easyscp</string>
    <key>CFBundleExecutable</key><string>EasySCP</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
EOF
# Ad-hoc signature so macOS will run the locally built bundle.
codesign --force --sign - "$app"

echo "Built $app"
