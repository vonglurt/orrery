#!/bin/bash
# Make an Ubuntu 24.04 machine a node of the fleet, in one run, the way
# columbia, calber and boburner were made by hand. Run ON THE NODE, as root,
# from the user who will run the fleet's jobs (user):
#
#   scp tools/fleet-node.sh tools/cuda-node.sh tools/fleet-login.sh tools/topnotch.sh \
#       tools/fleet-poweroff-setup.sh user@NODE:  &&  ssh -t user@NODE sudo bash fleet-node.sh
#
# In order, each step checking before it acts, so it is safe to run again:
#   1. the build tools: build-essential, pkg-config, git, curl
#   2. Rust for that user (rustup, stable, minimal), in their home: the
#      fleet's jobs build treadcore with it, and copal-tm is built with it
#   3. their checkouts: ~/code/treadcore (over HTTPS, as every node fetches
#      it) and the fleet's folder ~/fleet
#   4. with an NVIDIA GPU, cuda-node.sh: the driver kept or installed, CUDA,
#      the check, tmux and copal-tm. Without one: tmux, and copal-tm built
#      by the same step of cuda-node.sh is skipped, so it is built here
#   5. fleet-login.sh: the login banner (topnotch: CPU, memory, disk and the
#      GPU as they are now, and what runs over SSH)
#   6. fleet-poweroff-setup.sh: the fleet may power this machine off, and
#      run nothing else as root
# Then the lines for the coordinator: the host's entry in the queue's
# config.json, and its line in ~/.copal/fleets/FLEET/nodes. Output also goes
# to /var/log/fleet-node.log.
#
# Settings: ROLE (cuda or cpu; found from the PCI bus if not set).
set -euo pipefail
LOG=/var/log/fleet-node.log
exec > >(tee -a "$LOG") 2>&1
say()  { printf '\n== %s\n' "$*"; }
note() { printf '   %s\n' "$*"; }
die()  { printf '\nerror: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || die "run as root: sudo bash $0"
U="${SUDO_USER:-}"
[ -n "$U" ] && [ "$U" != root ] || die "run it with sudo from the user who will run the fleet's jobs"
HERE=$(cd "$(dirname "$0")" && pwd)
for f in cuda-node.sh fleet-login.sh topnotch.sh fleet-poweroff-setup.sh; do
    [ -f "$HERE/$f" ] || die "$f is not beside this script (copy all five, as the usage line says)"
done
. /etc/os-release
[ "${ID:-}" = ubuntu ] || die "this is ${PRETTY_NAME:-not Ubuntu}; a Copal (Alpine) node is set up by Copal itself"
as_user() { sudo -u "$U" -H bash -c "$1"; }

if [ -z "${ROLE:-}" ]; then
    if lspci 2>/dev/null | grep -qiE 'vga|3d' && lspci | grep -iE 'vga|3d' | grep -qi nvidia; then ROLE=cuda; else ROLE=cpu; fi
fi
say "$(hostname): a fleet node, role $ROLE, for $U ($PRETTY_NAME)"

# ---- 1. the build tools -------------------------------------------------------
say "1. The build tools"
_want=""
for p in build-essential pkg-config git curl tmux; do
    dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "ok installed" || _want="$_want $p"
done
if [ -n "$_want" ]; then
    apt-get update -qq
    # shellcheck disable=SC2086  # a list of package names
    apt-get install -y -qq $_want
    note "installed:$_want"
else
    note "all there"
fi

# ---- 2. Rust for the user -----------------------------------------------------
say "2. Rust for $U"
if as_user '[ -x ~/.cargo/bin/cargo ]'; then
    as_user '. ~/.cargo/env; rustup update -q stable >/dev/null 2>&1 || true'
else
    as_user 'curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable'
fi
note "$(as_user '. ~/.cargo/env; rustc --version; cargo --version' | tr '\n' ';')"

# ---- 3. the checkouts -----------------------------------------------------------
say "3. treadcore, and the fleet's folder"
as_user 'set -e; mkdir -p ~/code ~/fleet ~/.local/bin
if [ -d ~/code/treadcore/.git ]; then git -C ~/code/treadcore pull -q --ff-only || echo "   treadcore: not a fast-forward, left as it is"
else git clone -q https://github.com/treadcore/treadcore.git ~/code/treadcore; fi
echo "   treadcore at $(git -C ~/code/treadcore log --oneline -1 | cut -c1-60)"'

# ---- 4. CUDA, tmux and copal-tm ----------------------------------------------
if [ "$ROLE" = cuda ]; then
    say "4. CUDA: cuda-node.sh"
    bash "$HERE/cuda-node.sh"
else
    say "4. tmux and copal-tm (no NVIDIA GPU: no CUDA)"
    as_user 'set -e; . ~/.cargo/env
    if [ -d ~/code/copal-tm/.git ]; then git -C ~/code/copal-tm pull -q --ff-only || true
    else git clone -q https://github.com/vonglurt/copal-tm.git ~/code/copal-tm; fi
    cd ~/code/copal-tm && cargo build -q --release
    ln -sf ~/code/copal-tm/target/release/copal-tm ~/.local/bin/copal-tm'
    note "copal-tm: $(as_user '~/.local/bin/copal-tm --version')"
fi

# ---- 5. the login banner --------------------------------------------------------
say "5. The login banner (topnotch)"
( cd "$HERE" && bash fleet-login.sh --role "$ROLE" )

# ---- 6. power off ---------------------------------------------------------------
say "6. The fleet may power this machine off"
_po=$(USER_NAME="$U" sh "$HERE/fleet-poweroff-setup.sh" | tee /dev/stderr | grep -o '"poweroff": *"[^"]*"' || true)

# ---- for the coordinator ----------------------------------------------------------
_ip=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n1)
_caps='["cpu"]'
[ "$ROLE" = cuda ] && _caps='["cpu", "gpu"]'
say "Done. On the coordinator:"
note "the queue's config.json, in \"hosts\":"
note "  {\"name\": \"$(hostname)\", \"target\": \"$U@$_ip\", \"caps\": $_caps, \"nice\": 10${_po:+, $_po}}"
note "~/.copal/fleets/FLEET/nodes:"
note "  $(hostname)    $_ip"
note "Log: $LOG"
