#!/bin/sh
# Mount another fleet machine's share here, now and at every boot. orrery's
# Storage view stages it (storage.py stage-mount) and types it into this
# machine's Sessions tile, where the password is typed:
#
#   cd ~/orrery-recipes && doas sh mount-share.sh nfs 192.168.8.113:/Hexamir/assets /mnt/fleet/boburner-Hexamir-assets user
#   ... mount-share.sh KIND SOURCE MOUNTPOINT USER --undo      takes it away
#
# KIND:
#   nfs    NFS 4.2, nconnect=8 (several connections a mount, for a fast
#          link), noatime, hard, _netdev: the fleet's way between machines
#   sshfs  over SSH as USER with USER's key (allow_other, reconnect): the
#          way with nothing on the server but sshd, slower (encrypted, one
#          stream)
#   smb    SMB 3 with a credentials file /etc/fleet-smb-SERVER (username=,
#          password=, mode 600) that the person writes: the Mac's and the
#          Asustor's way
# At boot: an /etc/fstab line (Alpine: OpenRC's netmount mounts _netdev
# lines once the network is up; Ubuntu: systemd does). Run again, one line.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
KIND="${1:?nfs, sshfs or smb}"; SRC="${2:?server:/path}"; AT="${3:?the mount point}"; U="${4:-user}"
case "$AT" in /mnt/*|/media/*|/srv/*) ;; *) echo "a mount point under /mnt, /media or /srv" >&2; exit 1;; esac

if [ "${5:-}" = "--undo" ]; then
    umount "$AT" 2>/dev/null || umount -l "$AT" 2>/dev/null || true
    grep -v " $AT " /etc/fstab > /etc/fstab.tmp || true
    mv /etc/fstab.tmp /etc/fstab
    rmdir "$AT" 2>/dev/null || true
    echo "unmounted, and not at boot: $AT"
    exit 0
fi

pkg() { if command -v apk >/dev/null; then apk add --no-progress "$1"; else DEBIAN_FRONTEND=noninteractive apt-get -y install "$2"; fi; }
H=$(getent passwd "$U" | cut -d: -f6)
case "$KIND" in
    nfs)
        command -v mount.nfs >/dev/null || pkg nfs-utils nfs-common
        TYPE=nfs4; OPTS="vers=4.2,nconnect=8,hard,noatime,_netdev" ;;
    sshfs)
        command -v sshfs >/dev/null || pkg sshfs sshfs
        TYPE=fuse.sshfs
        OPTS="_netdev,reconnect,ServerAliveInterval=15,allow_other,default_permissions,IdentityFile=$H/.ssh/id_ed25519,uid=$(id -u "$U"),gid=$(id -g "$U")"
        SRC="$U@$SRC" ;;
    smb)
        command -v mount.cifs >/dev/null || pkg cifs-utils cifs-utils
        SERVER=$(echo "$SRC" | cut -d/ -f3)
        CRED=/etc/fleet-smb-$SERVER
        [ -f "$CRED" ] || { echo "write $CRED first (username=..., password=..., chmod 600)" >&2; exit 1; }
        TYPE=cifs; OPTS="vers=3.1.1,credentials=$CRED,uid=$(id -u "$U"),gid=$(id -g "$U"),_netdev,noatime" ;;
    *) echo "nfs, sshfs or smb" >&2; exit 1 ;;
esac

mkdir -p "$AT"
grep -v " $AT " /etc/fstab > /etc/fstab.tmp || true
printf '%s %s %s %s 0 0\n' "$SRC" "$AT" "$TYPE" "$OPTS" >> /etc/fstab.tmp
mv /etc/fstab.tmp /etc/fstab
command -v rc-update >/dev/null && rc-update add netmount default >/dev/null 2>&1 || true
command -v systemctl >/dev/null && systemctl daemon-reload || true
mountpoint -q "$AT" && umount "$AT" || true
mount "$AT"
echo "mounted: $SRC at $AT ($TYPE), and at every boot"
df -h "$AT" | tail -1
