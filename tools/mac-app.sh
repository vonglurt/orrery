#!/bin/sh
# mac-app.sh [ORRERY ARGS...] -- install orrery on a Mac: the binary into
# PREFIX/bin (default ~/.local) and Orrery.app into APPDIR (default
# ~/Applications), so the window opens from Finder, Launchpad, Spotlight and
# the Dock like any other application.
#
# The arguments are the ones the app starts orrery with; --gui is added if
# they leave it out. With none, the window opens on the fleet alone. The
# grove's console, for instance:
#
#   tools/mac-app.sh --operator paul --fleet grove \
#       --fleet-cmd "$HOME/.local/bin/copal fleet" \
#       --queue "$HOME/code/treadcore/build/queue/grove"
#
# Run it again with options to change them; with none (or `make install`)
# it keeps the last ones, from ~/.config/orrery/app-options.
# An app opened from Finder gets a bare PATH and no terminal, so the launcher
# puts ~/.local/bin and Homebrew back on the PATH and writes what orrery says
# to ~/Library/Logs/orrery.log. APPDIR=/Applications puts it there instead.
# --uninstall removes both.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=${PREFIX:-$HOME/.local}
APPDIR=${APPDIR:-$HOME/Applications}
APP="$APPDIR/Orrery.app"
[ "$(uname -s)" = Darwin ] || { echo "mac-app.sh: this is for macOS" >&2; exit 2; }

if [ "${1:-}" = --uninstall ]; then
    rm -rf "$APP" "$PREFIX/bin/orrery"
    echo "removed $APP and $PREFIX/bin/orrery"
    exit 0
fi

# One argument, quoted for sh.
q() { printf "'%s'" "$(printf %s "$1" | sed "s/'/'\\\\''/g")"; }
# The options are remembered: given none (a plain `make install`), the app
# keeps the last ones it was made with.
OPTS="${XDG_CONFIG_HOME:-$HOME/.config}/orrery/app-options"
if [ $# -eq 0 ] && [ -s "$OPTS" ]; then
    ARGS=$(cat "$OPTS")
else
    ARGS=""
    gui=0
    for a in "$@"; do
        [ "$a" = --gui ] && gui=1
        ARGS="$ARGS $(q "$a")"
    done
    [ $gui = 1 ] || ARGS=" --gui$ARGS"
    mkdir -p "$(dirname "$OPTS")"
    printf '%s' "$ARGS" > "$OPTS"
fi

cd "$ROOT"
# Under the memory guard: a release build beside a full census once ran
# this Mac out of memory (tools/mem-guard.sh).
sh "$ROOT/tools/mem-guard.sh" cargo build --release
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)

mkdir -p "$PREFIX/bin"
install -m 755 target/release/orrery "$PREFIX/bin/orrery"

# Build the bundle beside the old one and swap, so a failure leaves the old.
NEW="$APP.new"
rm -rf "$NEW"
mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
install -m 755 target/release/orrery "$NEW/Contents/MacOS/orrery"

# The launcher is not named Orrery: a Mac's disk ignores case, and it would
# be the binary.
cat > "$NEW/Contents/MacOS/launch" <<EOF
#!/bin/sh
# Orrery.app's launcher, written by $ROOT/tools/mac-app.sh.
export PATH="\$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:\$PATH"
mkdir -p "\$HOME/Library/Logs"
exec "\$(dirname "\$0")/orrery"$ARGS >>"\$HOME/Library/Logs/orrery.log" 2>&1
EOF
chmod 755 "$NEW/Contents/MacOS/launch"

cat > "$NEW/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Orrery</string>
    <key>CFBundleDisplayName</key><string>Orrery</string>
    <key>CFBundleIdentifier</key><string>org.copal.orrery</string>
    <key>CFBundleExecutable</key><string>launch</string>
    <key>CFBundleIconFile</key><string>orrery</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key><string>Orrery reaches the fleet's machines on the local network.</string>
</dict>
</plist>
EOF

# The icon: one drawing at 1024, scaled by sips into the iconset's ten sizes.
SET=$(mktemp -d)/orrery.iconset
mkdir -p "$SET"
python3 "$ROOT/tools/mac-icon.py" "$SET/icon_512x512@2x.png"
for s in 16 32 128 256 512; do
    sips -z $s $s "$SET/icon_512x512@2x.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    [ $s = 512 ] || sips -z $d $d "$SET/icon_512x512@2x.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$NEW/Contents/Resources/orrery.icns"
rm -rf "$(dirname "$SET")"

# Signed ad hoc, the binary first and then the bundle around it (codesign
# will not seal a bundle whose code inside is unsigned), so it opens from
# Finder without a complaint.
codesign --force --sign - "$NEW/Contents/MacOS/orrery"
codesign --force --sign - "$NEW"

mkdir -p "$APPDIR"
rm -rf "$APP"
mv "$NEW" "$APP"
# Tell Finder and Launchpad the icon changed.
touch "$APP"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" 2>/dev/null || true

echo "installed $PREFIX/bin/orrery and $APP"
echo "it starts: orrery$ARGS"
