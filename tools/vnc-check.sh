#!/bin/sh
# Run orrery's VNC authentication against a real x11vnc -- the server
# copal-remote starts on an X11 node, with the fleet's screen password.
#
# No display is needed: x11vnc serves a file as its framebuffer (-rawfb), and
# what is being proved is the handshake, DES and all. Run it where x11vnc is
# installed (apk add x11vnc), from the checkout:
#
#   sh tools/vnc-check.sh
set -e
D=$(mktemp -d)
trap 'kill "$pid" 2>/dev/null; rm -rf "$D"' EXIT INT TERM
head -c $((320 * 240 * 4)) /dev/zero > "$D/fb"
PW=$(tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 8)
( umask 077; printf '%s\n' "$PW" > "$D/passwd" )
PORT=5977
# x11vnc will not start inside a Wayland session unless told it is not one.
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 x11vnc -rawfb "map:$D/fb@320x240x32" \
    -rfbport "$PORT" -listen 127.0.0.1 -passwdfile "$D/passwd" -forever -quiet \
    > "$D/log" 2>&1 &
pid=$!
sleep 3
kill -0 "$pid" 2>/dev/null || { cat "$D/log"; echo "x11vnc did not start" >&2; exit 1; }
ORRERY_VNC_PORT=$PORT ORRERY_VNC_PASSWORD=$PW cargo test --release -q rfb::live -- --test-threads 1
