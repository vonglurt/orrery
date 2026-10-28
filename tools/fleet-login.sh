#!/bin/bash
# The login of a fleet node that is not Copal (an Ubuntu CUDA node, say):
# Copal fleet branding, what this node is for, hints, and none of Ubuntu's
# advertising. Run ON THE NODE, as root:
#
#   scp tools/fleet-login.sh tools/fleet-status.sh user@NODE:  &&  ssh -t user@NODE sudo bash fleet-login.sh --role cuda
#   ssh -t user@NODE sudo bash fleet-login.sh --undo        # put Ubuntu's back
#
# Ubuntu builds its login message from the scripts in /etc/update-motd.d/.
# This turns off the ones that sell or distract -- the help links, the news
# fetched from motd.ubuntu.com, Ubuntu Pro's ESM advertising, the "New
# release available" notice -- and keeps the ones that tell the person
# something about this machine: load, memory, temperature and address
# (landscape-sysinfo), updates waiting, a reboot required, the HWE kernel's end
# of life. It adds 01-copal-fleet, which runs at each login: the fleet's name,
# this node and its role, the GPU as it is now, and hints.
#
# Turned off is chmod -x (run-parts skips a script that is not executable);
# --undo turns every script in SILENCE back on. No list of what was done is
# kept: a list written at the end is lost when a run stops partway, which is
# how the first run on calber left five scripts off and nothing to undo them. Settings live in
# /etc/copal-fleet/node.conf, which 01-copal-fleet reads, so a node's role
# and hints can be edited without running this again.
#
# --role cuda also holds the release: Prompt=never in
# /etc/update-manager/release-upgrades, because Ubuntu 26.04 has no CUDA 12
# packages and a Pascal card needs CUDA 12 (docs/pascal-versions-lab-report.md).
set -euo pipefail

ROLE=''
UNDO=0
while [ $# -gt 0 ]; do
    case "$1" in
        --role) ROLE="${2:?--role needs a value}"; shift 2 ;;
        --undo) UNDO=1; shift ;;
        *) echo "usage: fleet-login.sh [--role cuda|NAME] | --undo" >&2; exit 2 ;;
    esac
done
[ "$(id -u)" = 0 ] || { echo "run as root: sudo bash $0 ..." >&2; exit 1; }

D=/etc/copal-fleet
M=/etc/update-motd.d
# 50-landscape-sysinfo too: fleet-status shows load, memory, disk and address,
# as bars, so Ubuntu's lines would say it twice.
SILENCE='10-help-text 50-landscape-sysinfo 50-motd-news 91-contract-ua-esm-status 91-release-upgrade'

if [ "$UNDO" = 1 ]; then
    for f in $SILENCE; do [ -e "$M/$f" ] && chmod +x "$M/$f" && echo "on again: $f"; done
    rm -f "$D"/motd-silenced*
    rm -f "$M/01-copal-fleet" /usr/local/bin/fleet-status
    [ -f /etc/default/motd-news ] && sed -i 's/^ENABLED=0/ENABLED=1/' /etc/default/motd-news
    command -v pro >/dev/null 2>&1 && pro config set apt_news=true >/dev/null 2>&1 || true
    echo "Ubuntu's login message is back. (Prompt= in /etc/update-manager/release-upgrades is left as it is.)"
    exit 0
fi

mkdir -p "$D"

# ---- silence -----------------------------------------------------------------
for f in $SILENCE; do
    if [ -x "$M/$f" ]; then chmod -x "$M/$f"; echo "silenced: $f"; fi
done
rm -f "$D"/motd-silenced*   # left by the first version
# The news is also fetched by a timer; off at the source.
[ -f /etc/default/motd-news ] && sed -i 's/^ENABLED=1/ENABLED=0/' /etc/default/motd-news
# Ubuntu Pro's messages in apt's own output ("Enable ESM Apps ...").
command -v pro >/dev/null 2>&1 && pro config set apt_news=false >/dev/null 2>&1 && echo "silenced: apt_news (Ubuntu Pro)" || true

# ---- the node's settings ------------------------------------------------------
if [ ! -f "$D/node.conf" ] || [ -n "$ROLE" ]; then
    case "${ROLE:-node}" in
        cuda)
            cat > "$D/node.conf" <<'CONF'
# Read at each login by /etc/update-motd.d/01-copal-fleet (orrery's
# tools/fleet-login.sh). Edit freely; HINT lines are shown in order.
ROLE="the fleet's CUDA node"
HOLD="Ubuntu 24.04, driver 580, CUDA 12.9: held for a Pascal card"
HINT="nvidia-smi                       the card, now"
HINT="nvcc --version                   the CUDA toolkit"
HINT="~/code/burnprobe                 Burn on this card (orrery tools/burn-probe)"
HINT="sudo less /var/log/cuda-node.log how this node was set up"
WARN="Do not run do-release-upgrade: Ubuntu 26.04 has no CUDA 12 for this card."
CONF
            # Hold the release (see the header).
            if [ -f /etc/update-manager/release-upgrades ]; then
                sed -i 's/^Prompt=.*/Prompt=never/' /etc/update-manager/release-upgrades
                echo "release upgrades: Prompt=never"
            fi ;;
        *)
            cat > "$D/node.conf" <<CONF
# Read at each login by /etc/update-motd.d/01-copal-fleet (orrery's
# tools/fleet-login.sh). Edit freely; HINT lines are shown in order.
ROLE="${ROLE:-a fleet node}"
CONF
            ;;
    esac
    echo "wrote $D/node.conf"
fi

# ---- the banner: fleet-status, at each login ----------------------------------
# fleet-status (tools/fleet-status.sh, copied beside this script) is the
# panel: the node, CPU/memory/disk bars, the GPU and what runs on it, who is
# in over SSH and what each connection started, the busiest processes, the
# hints. Installed as a command too: run it any time, or watch -c fleet-status.
_here=$(cd "$(dirname "$0")" && pwd)
[ -f "$_here/fleet-status.sh" ] || { echo "fleet-status.sh is not beside $0: copy both (see the header)" >&2; exit 1; }
install -m 0755 "$_here/fleet-status.sh" /usr/local/bin/fleet-status
echo "installed /usr/local/bin/fleet-status"
cat > "$M/01-copal-fleet" <<'MOTD'
#!/bin/sh
# Copal fleet login banner (orrery's tools/fleet-login.sh): the fleet-status
# panel. Settings: /etc/copal-fleet/node.conf. Remove: sudo bash fleet-login.sh --undo
[ -x /usr/local/bin/fleet-status ] && exec /usr/local/bin/fleet-status
MOTD
chmod +x "$M/01-copal-fleet"
echo "wrote $M/01-copal-fleet"

echo
echo "The next login shows it. To see it now:  run-parts $M"
