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
# The package manager's own update and upgrade, nothing else. A kernel
# upgraded waits for the next boot, and the script says so rather than
# rebooting: a machine running a census is not restarted by a button.
# On Ubuntu the NVIDIA driver is held to its branch by the package names
# cuda-node.sh chose; apt upgrade keeps within them.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
before=$(uname -r)

if command -v apk >/dev/null; then
    apk update
    apk upgrade --no-progress
    new=$(ls -t /lib/modules 2>/dev/null | head -1)
elif command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get -y upgrade
    new=$(ls -t /lib/modules 2>/dev/null | head -1)
else
    echo "no apk or apt here" >&2
    exit 1
fi

if [ -n "${new:-}" ] && [ "$new" != "$before" ]; then
    echo "a newer kernel ($new) is installed; the running one is $before: it takes effect at the next boot"
else
    echo "up to date; the kernel is $before"
fi
