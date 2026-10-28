#!/bin/bash
# The login of a fleet node that is not Copal (an Ubuntu CUDA node, say):
# Copal fleet branding, what this node is for, hints, and none of Ubuntu's
# advertising. Run ON THE NODE, as root:
#
#   scp tools/fleet-login.sh user@NODE:  &&  ssh -t user@NODE sudo bash fleet-login.sh --role cuda
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
# Turned off is chmod -x, listed in /etc/copal-fleet/motd-silenced, so
# --undo turns back on exactly what this turned off. Settings live in
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
SILENCE='10-help-text 50-motd-news 91-contract-ua-esm-status 91-release-upgrade'

if [ "$UNDO" = 1 ]; then
    if [ -f "$D/motd-silenced" ]; then
        while read -r f; do [ -f "$M/$f" ] && chmod +x "$M/$f" && echo "on again: $f"; done < "$D/motd-silenced"
        rm -f "$D/motd-silenced"
    fi
    rm -f "$M/01-copal-fleet"
    [ -f /etc/default/motd-news ] && sed -i 's/^ENABLED=0/ENABLED=1/' /etc/default/motd-news
    command -v pro >/dev/null 2>&1 && pro config set apt_news=true >/dev/null 2>&1 || true
    echo "Ubuntu's login message is back. (Prompt= in /etc/update-manager/release-upgrades is left as it is.)"
    exit 0
fi

mkdir -p "$D"

# ---- silence -----------------------------------------------------------------
: > "$D/motd-silenced.new"
for f in $SILENCE; do
    if [ -x "$M/$f" ]; then
        chmod -x "$M/$f"; echo "$f" >> "$D/motd-silenced.new"; echo "silenced: $f"
    fi
done
# Keep what an earlier run silenced, so --undo still knows it.
cat "$D/motd-silenced" "$D/motd-silenced.new" 2>/dev/null | sort -u > "$D/motd-silenced.all"
mv "$D/motd-silenced.all" "$D/motd-silenced"; rm -f "$D/motd-silenced.new"
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

# ---- the banner ---------------------------------------------------------------
cat > "$M/01-copal-fleet" <<'MOTD'
#!/bin/bash
# Copal fleet login banner (orrery's tools/fleet-login.sh). Settings:
# /etc/copal-fleet/node.conf. Remove with: sudo bash fleet-login.sh --undo
ROLE=''; HOLD=''; WARN=''; HINTS=()
if [ -r /etc/copal-fleet/node.conf ]; then
    while IFS= read -r line; do
        case "$line" in
            ROLE=*) ROLE=${line#ROLE=}; ROLE=${ROLE//\"/} ;;
            HOLD=*) HOLD=${line#HOLD=}; HOLD=${HOLD//\"/} ;;
            WARN=*) WARN=${line#WARN=}; WARN=${WARN//\"/} ;;
            HINT=*) h=${line#HINT=}; HINTS+=("${h//\"/}") ;;
        esac
    done < /etc/copal-fleet/node.conf
fi
. /etc/os-release
printf '\n'
printf '  \033[1mCopal fleet\033[0m \033[2m· orrery\033[0m\n'
printf '  \033[1m%s\033[0m -- %s \033[2m(%s, not Copal)\033[0m\n' "$(hostname)" "${ROLE:-a fleet node}" "$PRETTY_NAME"
if command -v nvidia-smi >/dev/null 2>&1; then
    g=$(nvidia-smi --query-gpu=name,driver_version,temperature.gpu,utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1)
    if [ -n "$g" ]; then
        IFS=',' read -r name drv temp util mu mt <<<"$g"
        printf '  GPU   %s · driver%s · %s°C · %s%% busy · %s of%s MiB\n' "$name" "$drv" "${temp# }" "${util# }" "${mu# }" "$mt"
    else
        printf '  GPU   \033[33mnvidia-smi gives no answer: is the driver loaded?\033[0m\n'
    fi
fi
for c in /usr/local/cuda/bin/nvcc /usr/local/cuda-*/bin/nvcc; do
    [ -x "$c" ] || continue
    v=$("$c" --version 2>/dev/null | sed -n 's/.*release \([0-9.]*\).*/\1/p')
    printf '  CUDA  %s\n' "${v:-?}"; break
done
[ -n "$HOLD" ] && printf '  \033[2mheld: %s\033[0m\n' "$HOLD"
if [ "${#HINTS[@]}" -gt 0 ]; then
    printf '\n  Hints\n'
    for h in "${HINTS[@]}"; do printf '    %s\n' "$h"; done
fi
[ -n "$WARN" ] && printf '\n  \033[33m%s\033[0m\n' "$WARN"
printf '\n'
MOTD
chmod +x "$M/01-copal-fleet"
echo "wrote $M/01-copal-fleet"

echo
echo "The next login shows it. To see it now:  run-parts $M"
