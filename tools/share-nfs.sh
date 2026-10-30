#!/bin/sh
# Share a directory with the fleet over NFS, now and at every boot. orrery's
# Storage view stages it (storage.py stage-share) and types it into the
# machine's Sessions tile, where the password is typed:
#
#   cd ~/orrery-recipes && doas sh share-nfs.sh /Hexamir/assets 192.168.8.0/24
#   ... share-nfs.sh PATH NET --undo         takes the share away
#
# NFS, because the fleet's links are the bottleneck and the CPUs are busy:
# the kernel's own server, no encryption on a trusted LAN, and with
# nconnect several TCP connections a mount (docs/storage-lab-report.md).
# The export is rw, sync, root squashed (a client's root is nobody here),
# to the fleet's network only. Alpine: nfs-utils and OpenRC's nfs service;
# Ubuntu: nfs-kernel-server. Run again, it writes the same line once.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
P="${1:?the directory to share}"
NET="${2:-192.168.8.0/24}"
case "$P" in /*) ;; *) echo "an absolute path" >&2; exit 1;; esac
[ "$P" = / ] && { echo "not the whole system" >&2; exit 1; }
LINE="$P $NET(rw,sync,no_subtree_check,root_squash)"
touch /etc/exports

if [ "${3:-}" = "--undo" ]; then
    grep -v "^$P " /etc/exports > /etc/exports.tmp || true
    mv /etc/exports.tmp /etc/exports
    exportfs -ra 2>/dev/null || true
    echo "no longer shared: $P"
    exit 0
fi

[ -d "$P" ] || { echo "no directory $P here" >&2; exit 1; }
if command -v apk >/dev/null; then
    command -v exportfs >/dev/null || apk add --no-progress nfs-utils
    rc-update add nfs default >/dev/null 2>&1 || true
    rc-service nfs status >/dev/null 2>&1 || rc-service nfs start
elif command -v apt-get >/dev/null; then
    command -v exportfs >/dev/null || DEBIAN_FRONTEND=noninteractive apt-get -y install nfs-kernel-server
    systemctl enable --now nfs-server >/dev/null
else
    echo "no apk or apt here" >&2; exit 1
fi
# One line a path: a path shared again is shared as this line says.
grep -v "^$P " /etc/exports > /etc/exports.tmp || true
echo "$LINE" >> /etc/exports.tmp
mv /etc/exports.tmp /etc/exports
exportfs -ra
echo "shared: $LINE"
echo "mount it on another machine: storage.py QUEUE stage-mount nfs $(hostname) $P CLIENT"
