#!/bin/sh
# Run orrery's test suite against a real Wayland compositor.
#
# The tests in src/wl.rs that need a compositor step aside when there is none,
# which keeps `cargo test` green on a developer's Mac and also means they prove
# nothing there. This runs them where they mean something. weston headless is
# the stand-in for Hyprland: same xdg-shell, no display needed.
#
#   docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
#       -e CARGO_TARGET_DIR=/tmp/t rust:alpine sh /w/tools/wl-check.sh
#
# On a node with a session already running, WAYLAND_DISPLAY is set and the
# weston half is unnecessary -- `cargo test` alone does it.
set -e
apk add --no-cache musl-dev weston weston-backend-headless weston-shell-desktop >/dev/null 2>&1
mkdir -p /tmp/xdg && chmod 700 /tmp/xdg
export XDG_RUNTIME_DIR=/tmp/xdg
weston --backend=headless --socket=wayland-1 --width=960 --height=600 >/tmp/weston.log 2>&1 &
i=0; while [ ! -S /tmp/xdg/wayland-1 ] && [ $i -lt 40 ]; do sleep 0.25; i=$((i+1)); done
export WAYLAND_DISPLAY=wayland-1
cargo test 2>&1 | grep -E "^test wl::|test result|running|note:|skipped" | tail -30
echo "=== weston complaints ==="
grep -iE "error in client|invalid|protocol err" /tmp/weston.log | tail -5 || echo "(none)"
