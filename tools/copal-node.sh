#!/bin/sh
# Make a Copal (Alpine) machine a fleet node: fleet-node.sh's work, for
# Alpine. Paul, 29 October, two machines with Copal's full install added
# to orrery: "create a script for configuring these computer after they
# are added to orrery."
#
# Run it ON THE MACHINE, as root (orrery's Machines view stages it and
# types this into the machine's Sessions tile, where the password is
# typed):
#
#   cd ~/orrery-recipes && doas sh copal-node.sh
#
# What it does, each step checked first, so running it again is safe:
#   1. the build tools from Alpine's own repositories, those not there:
#      build-base, git, curl, tmux, rust and cargo (Copal's full install
#      has them; a smaller one may not);
#   2. as the user, not root: ~/code/treadcore over HTTPS, and ~/fleet;
#   3. the power-off rule (fleet-poweroff-setup.sh: the user may run
#      poweroff as root without a password, and nothing else).
# Nothing else on the machine is changed: no service, no firewall rule,
# no SSH setting. Copal's lockdown is left as it is.
set -eu

U="${USER_NAME:-user}"
[ "$(id -u)" = 0 ] || { echo "run as root (doas sh copal-node.sh)" >&2; exit 1; }
id "$U" >/dev/null 2>&1 || { echo "no user $U here" >&2; exit 1; }
command -v apk >/dev/null || { echo "not Alpine: on Ubuntu, fleet-node.sh" >&2; exit 1; }
HOME_U=$(getent passwd "$U" | cut -d: -f6)
HERE=$(cd "$(dirname "$0")" && pwd)

# 1. The tools, from Alpine's repositories: only what is missing.
want=""
for p in build-base git curl tmux rust cargo; do
    apk info -e "$p" >/dev/null 2>&1 || want="$want $p"
done
if [ -n "$want" ]; then
    echo "installing:$want"
    apk add --no-progress $want
else
    echo "the build tools are there"
fi

# 2. The checkout and the fleet's directory, as the user.
su -s /bin/sh "$U" -c '
    set -e
    mkdir -p ~/code ~/fleet
    if [ -d ~/code/treadcore/.git ]; then
        echo "treadcore: there ($(git -C ~/code/treadcore log --oneline -1 | cut -c1-50))"
    else
        git clone -q https://github.com/treadcore/treadcore.git ~/code/treadcore
        echo "treadcore: cloned ($(git -C ~/code/treadcore log --oneline -1 | cut -c1-50))"
    fi
'

# 3. The power-off rule.
if [ -e /etc/doas.d/fleet-poweroff.conf ]; then
    echo "the power-off rule is there"
else
    USER_NAME="$U" sh "$HERE/fleet-poweroff-setup.sh"
fi

echo "done: probe it in orrery's Machines view, then Join"
echo "($HOME_U is the user's home; nothing outside it and /etc/doas.d was changed but the packages)"
