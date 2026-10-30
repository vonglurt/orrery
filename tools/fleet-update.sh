#!/bin/sh
# Bring a machine's packages up to date: Alpine's apk, or Ubuntu's apt.
# Paul, 29 October: "add more like apk update, apk upgrade ... add those
# buttons."
#
# Run it ON THE MACHINE, as root (orrery's Machines view stages it and
# types it into the machine's Sessions tile, where the password is typed):
#
#   cd ~/orrery-recipes && doas sh fleet-update.sh      (Alpine, Copal)
#   cd ~/orrery-recipes && sudo sh fleet-update.sh      (Ubuntu)
#
# --auto is how the daily job and the boot run call it (fleet-auto-update.sh
# installs them): the same, but on Ubuntu the NVIDIA driver and CUDA are held
# back. A driver upgraded under a running machine is a driver whose library
# no longer matches the module loaded, and every CUDA program fails until
# the boot ("Driver/library version mismatch"): crucible's lanes would stop
# overnight for an update no one asked for that day. By hand (no --auto)
# everything is upgraded.
#
# The package manager's own update and upgrade, nothing else. A kernel
# upgraded waits for the next boot, and the script says so rather than
# rebooting: a machine running a census is not restarted by a button or a
# timer. Each run ends with one line in /var/log/fleet-update.log, which
# orrery's probe reads.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
AUTO=0
[ "${1:-}" = "--auto" ] && AUTO=1
LOG=/var/log/fleet-update.log
before=$(uname -r)
held=""

say() {
    echo "$1"
    printf '%s %s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "$([ $AUTO = 1 ] && echo auto || echo by-hand)" "$1" >> "$LOG"
}
trap 'say "failed (exit $?): run it by hand to see why"' EXIT
# Only the last line matters to the probe; the log is kept to a year of days.
if [ -f "$LOG" ]; then tail -n 400 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"; fi

if command -v apk >/dev/null; then
    apk update
    apk upgrade --no-progress
    kind=apk
elif command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    # apt's own daily job may hold the lock: wait for it rather than fail.
    APT="apt-get -o DPkg::Lock::Timeout=900"
    $APT update
    if [ $AUTO = 1 ]; then
        # Held for this run only; one held already stays held.
        for p in $(dpkg-query -W -f '${Package}\n' 2>/dev/null | grep -E '^(nvidia-|libnvidia-|cuda-|libcuda|linux-modules-nvidia-)' || true); do
            if ! apt-mark showhold | grep -qx "$p"; then
                apt-mark hold "$p" >/dev/null && held="$held $p"
            fi
        done
    fi
    if ! $APT -y upgrade; then
        if [ -n "$held" ]; then apt-mark unhold $held >/dev/null; fi
        exit 1
    fi
    if [ -n "$held" ]; then apt-mark unhold $held >/dev/null; fi
    kind=apt
else
    echo "no apk or apt here" >&2
    exit 1
fi

trap - EXIT
new=$(ls -t /lib/modules 2>/dev/null | head -1)
n=$(echo $held | wc -w)
also=""
if [ "$n" -gt 0 ]; then also="; the NVIDIA driver and CUDA held back for a by-hand run ($n held)"; fi
if [ -n "${new:-}" ] && [ "$new" != "$before" ]; then
    say "$kind upgraded; a newer kernel ($new) waits for the next boot, $before runs now$also"
else
    say "$kind up to date; the kernel is $before$also"
fi
