#!/bin/sh
# Keep a machine up to date by itself: fleet-update.sh --auto every day, and
# a few minutes after each boot. Paul, 29 October: "A script that does it
# every day, and inserts it into the startup ... I just want always daily
# uptodate. Or maybe on startup." Both, then: the daily run for a machine
# that stays up, the boot run for one that was off when the day's came.
#
# Run it ON THE MACHINE, once, as root (orrery's Machines view: the recipe
# "Keep it up to date"):
#
#   cd ~/orrery-recipes && doas sh fleet-auto-update.sh      (Alpine, Copal)
#   cd ~/orrery-recipes && sudo sh fleet-auto-update.sh      (Ubuntu)
#   ... fleet-auto-update.sh --undo                          takes it all away
#
# It puts fleet-update.sh at /usr/local/sbin/fleet-update.sh, then:
#   Alpine:  /etc/periodic/daily/fleet-update, run by busybox crond at 2:00
#            (crond added to the default runlevel and started), and
#            /etc/local.d/fleet-update.start, run by OpenRC's local service
#            at boot (added too), two minutes after, in the background.
#   Ubuntu:  fleet-update.service and fleet-update.timer in /etc/systemd/
#            system: daily at a random time within the hour after 3:00,
#            five minutes after boot, and a day missed while off made up at
#            the next start (Persistent).
# Run again, it writes the same files. Each run's line is in
# /var/log/fleet-update.log.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=/usr/local/sbin/fleet-update.sh

if [ "${1:-}" = "--undo" ]; then
    rm -f /etc/periodic/daily/fleet-update /etc/local.d/fleet-update.start
    if command -v systemctl >/dev/null; then
        systemctl disable --now fleet-update.timer 2>/dev/null || true
        rm -f /etc/systemd/system/fleet-update.timer /etc/systemd/system/fleet-update.service
        systemctl daemon-reload
    fi
    rm -f "$BIN"
    echo "taken away: no daily or boot update (the log stays at /var/log/fleet-update.log)"
    exit 0
fi

[ -f "$HERE/fleet-update.sh" ] || { echo "fleet-update.sh is not beside this script" >&2; exit 1; }
mkdir -p "$(dirname "$BIN")" /var/log
install -m 0755 "$HERE/fleet-update.sh" "$BIN"
touch /var/log/fleet-update.log && chmod 0644 /var/log/fleet-update.log

if command -v apk >/dev/null && command -v rc-update >/dev/null; then
    mkdir -p /etc/periodic/daily /etc/local.d
    printf '#!/bin/sh\n# orrery tools/fleet-auto-update.sh: the daily update.\nexec %s --auto >/dev/null 2>&1\n' "$BIN" > /etc/periodic/daily/fleet-update
    chmod 0755 /etc/periodic/daily/fleet-update
    printf '#!/bin/sh\n# orrery tools/fleet-auto-update.sh: the update after a boot, when the network is up.\n( sleep 120; %s --auto ) >/dev/null 2>&1 &\n' "$BIN" > /etc/local.d/fleet-update.start
    chmod 0755 /etc/local.d/fleet-update.start
    # Alpine's root crontab runs the daily folder at 2:00; a crontab without
    # that line (a system that changed it) is given Alpine's own line back.
    if ! grep -q 'periodic/daily' /etc/crontabs/root 2>/dev/null; then
        mkdir -p /etc/crontabs
        printf '0\t2\t*\t*\t*\trun-parts /etc/periodic/daily\n' >> /etc/crontabs/root
        echo "the root crontab had no daily line: Alpine's added"
    fi
    rc-update add crond default >/dev/null 2>&1 || true
    rc-update add local default >/dev/null 2>&1 || true
    rc-service crond status >/dev/null 2>&1 || rc-service crond start >/dev/null 2>&1 || true
    echo "daily: /etc/periodic/daily/fleet-update (crond, 2:00); at boot: /etc/local.d/fleet-update.start"
elif command -v systemctl >/dev/null; then
    cat > /etc/systemd/system/fleet-update.service <<EOF
[Unit]
Description=orrery: the system's packages brought up to date (fleet-update.sh --auto)
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=$BIN --auto
Nice=10
EOF
    cat > /etc/systemd/system/fleet-update.timer <<EOF
[Unit]
Description=orrery: the packages updated daily and after each boot

[Timer]
OnCalendar=*-*-* 03:00:00
RandomizedDelaySec=1h
OnBootSec=5min
Persistent=true

[Install]
WantedBy=timers.target
EOF
    systemctl daemon-reload
    systemctl enable --now fleet-update.timer
    echo "daily: fleet-update.timer (3:00 and an hour's spread; 5 minutes after a boot; a missed day made up)"
else
    echo "neither OpenRC (Alpine) nor systemd (Ubuntu) here: nothing installed" >&2
    exit 1
fi
echo "the first run is tonight, or at the next boot; to run one now: $BIN"
