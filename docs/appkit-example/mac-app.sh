#!/bin/sh
# mac-app.sh -- build orbit and make Orbit.app in APPDIR (default
# ~/Applications): opened from Finder, Launchpad, Spotlight and the Dock.
#
# A bundle is a folder with a fixed shape:
#   Orbit.app/Contents/Info.plist      what it is, and which file to run
#   Orbit.app/Contents/MacOS/orbit     that file
#   Orbit.app/Contents/Resources/      its icon, if it has one
# orbit needs no arguments, so the binary itself is the bundle's executable.
# A program that does (orrery) puts a small launcher script there instead,
# named differently from the binary: a Mac's disk ignores case.
set -eu
cd "$(dirname "$0")"
APPDIR=${APPDIR:-$HOME/Applications}
APP="$APPDIR/Orbit.app"

cargo build --release

NEW="$APP.new"
rm -rf "$NEW"
mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
install -m 755 target/release/orbit "$NEW/Contents/MacOS/orbit"
cat > "$NEW/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Orbit</string>
    <key>CFBundleIdentifier</key><string>org.example.orbit</string>
    <key>CFBundleExecutable</key><string>orbit</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# An icon, if there is a 1024-pixel PNG beside this script: sips makes the
# iconset's sizes, iconutil the .icns, and Info.plist names it.
if [ -f icon.png ]; then
    SET=$(mktemp -d)/orbit.iconset
    mkdir -p "$SET"
    for s in 16 32 128 256 512; do
        sips -z $s $s icon.png --out "$SET/icon_${s}x${s}.png" >/dev/null
        sips -z $((s * 2)) $((s * 2)) icon.png --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$SET" -o "$NEW/Contents/Resources/orbit.icns"
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string orbit" "$NEW/Contents/Info.plist"
fi
# Ad hoc: no certificate. The binary, then the bundle that seals it.
codesign --force --sign - "$NEW/Contents/MacOS/orbit"
codesign --force --sign - "$NEW"

mkdir -p "$APPDIR"
rm -rf "$APP"
mv "$NEW" "$APP"
echo "made $APP"
