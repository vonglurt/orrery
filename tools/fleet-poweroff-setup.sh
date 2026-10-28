#!/bin/sh
# Let the fleet power this machine off, and nothing else (fleet-queue's
# shutdown-now; docs/fleet-train.md section 5).
#
# Run it ON THE MACHINE, once, as root:
#
#   scp tools/fleet-poweroff-setup.sh user@NODE:  &&  ssh -t user@NODE doas sh fleet-poweroff-setup.sh   (Alpine, Copal)
#   scp tools/fleet-poweroff-setup.sh user@NODE:  &&  ssh -t user@NODE sudo sh fleet-poweroff-setup.sh   (Ubuntu)
#
# It writes one rule: the user named (USER_NAME=, default "user") may run
# the power-off command as root without a password. No other command is
# allowed by it. It prints the "poweroff" line for the host's entry in the
# queue's config.json. Run again, it writes the same rule. To take it away:
# delete /etc/doas.d/fleet-poweroff.conf or /etc/sudoers.d/fleet-poweroff.
set -eu

U="${USER_NAME:-user}"
[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
id "$U" >/dev/null 2>&1 || { echo "no user $U here" >&2; exit 1; }

if [ -d /etc/doas.d ] && command -v doas >/dev/null; then
    P=$(command -v poweroff || echo /sbin/poweroff)
    f=/etc/doas.d/fleet-poweroff.conf
    printf '# Written by orrery tools/fleet-poweroff-setup.sh: the fleet may power this machine off.\npermit nopass %s as root cmd %s\n' "$U" "$P" > "$f.tmp"
    chmod 0400 "$f.tmp"
    doas -C "$f.tmp" >/dev/null 2>&1 || { rm -f "$f.tmp"; echo "doas rejected the rule; nothing changed" >&2; exit 1; }
    mv "$f.tmp" "$f"
    echo "wrote $f"
    echo "config.json, this host: \"poweroff\": \"doas -n $P\""
elif [ -d /etc/sudoers.d ] && command -v sudo >/dev/null; then
    P=$(command -v systemctl || true)
    if [ -n "$P" ]; then C="$P poweroff"; else C=$(command -v poweroff || echo /sbin/poweroff); fi
    f=/etc/sudoers.d/fleet-poweroff
    printf '# Written by orrery tools/fleet-poweroff-setup.sh: the fleet may power this machine off.\n%s ALL=(root) NOPASSWD: %s\n' "$U" "$C" > "$f.tmp"
    chmod 0440 "$f.tmp"
    visudo -cf "$f.tmp" >/dev/null || { rm -f "$f.tmp"; echo "visudo rejected the rule; nothing changed" >&2; exit 1; }
    mv "$f.tmp" "$f"
    echo "wrote $f"
    echo "config.json, this host: \"poweroff\": \"sudo -n $C\""
else
    echo "neither doas (with /etc/doas.d) nor sudo (with /etc/sudoers.d) here; nothing changed" >&2
    exit 1
fi
