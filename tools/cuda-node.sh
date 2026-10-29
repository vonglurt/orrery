#!/bin/bash
# Make an Ubuntu 24.04 or 26.04 machine with an NVIDIA GPU a CUDA node of the fleet.
#
# Run it ON THE NODE, as root. From the console's machine:
#
#   scp tools/cuda-node.sh user@NODE:  &&  ssh -t user@NODE sudo bash cuda-node.sh
#
# Safe to run again: every step checks before it acts. Everything it prints
# also goes to /var/log/cuda-node.log.
#
# WHY UBUNTU, AND WHICH ONE: THE CARD SAYS. CUDA needs NVIDIA's own driver,
# whose user space is built for glibc; a Copal node is Alpine (musl), so a
# CUDA machine runs Ubuntu and joins the fleet over SSH.
#   - A card before Turing (Maxwell, Pascal, Volta: compute under 7.5; the
#     GTX 900 and 10 series): the 580 driver series is its last, and CUDA 12
#     its last toolkit. NVIDIA's repository for 26.04 has only CUDA 13, its
#     24.04 one CUDA 12.9: so 24.04 (calber, boburner, whitewolf). Copal's
#     boot menu has "Install Ubuntu 24.04 LTS Server (network): for NVIDIA
#     Pascal + CUDA 12".
#   - Turing or newer (RTX 20 series on; saskatchewan's RTX 4060 Ti is Ada,
#     8.9): 26.04, CUDA 13.4, Ubuntu's 595-server branch. 24.04 works too.
# The release's own settings are in one table (RELEASES, below); the card's
# generation is read from its PCI id before any driver is there, and a card
# too old for the release stops the run before anything is installed.
#
# WHAT IT DOES, in order:
#   1. checks: root, Ubuntu 24.04 or 26.04, an NVIDIA GPU, and that the card
#      and the release go together
#   2. the driver: keeps Ubuntu's branch if installed (the installer's
#      "third-party" choice puts one in, with Ubuntu's prebuilt signed
#      modules), or installs the release's; adds nvidia-utils for nvidia-smi
#   3. NVIDIA's CUDA repository, by its keyring package (sha256 pinned), at a
#      LOW apt priority, so none of its driver packages can ever replace
#      Ubuntu's: from it only what is asked for by name is installed
#   4. the CUDA toolkit (the release's: 12.9 on 24.04, 13.4 on 26.04)
#   5. PATH and the library path for every login
#   6. the check: nvidia-smi, then a small CUDA program compiled for this
#      card and run: device properties and a million-element vector add
#   7. the fleet's own tools: tmux (orrery's Sessions keep a session on each
#      machine in it), and copal-tm (the task manager a session opens with:
#      it shows the card's load and CUDA jobs), built for the user who ran
#      sudo, from github.com/vonglurt/copal-tm, linked into ~/.local/bin
#
# Settings: DRIVER= (the Ubuntu branch), CUDA= (the toolkit), each the
# release's unless set.
set -euo pipefail

LOG=/var/log/cuda-node.log
exec > >(tee -a "$LOG") 2>&1
say()  { printf '\n==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die()  { printf '\nerror: %s\n' "$*" >&2; exit 1; }
trap '_e=$?; [ "$BASH_SUBSHELL" = 0 ] && printf "\ncuda-node.sh: STOPPED at line %s (exit %s): %s\n" "$LINENO" "$_e" "$BASH_COMMAND"' ERR
export DEBIAN_FRONTEND=noninteractive

say "cuda-node.sh, $(date -u +%Y-%m-%dT%H:%M:%SZ), on $(hostname)"

# ---- 1. checks --------------------------------------------------------------
[ "$(id -u)" = 0 ] || die "run as root: sudo bash $0"
. /etc/os-release
[ "${ID:-}" = ubuntu ] || die "this is ${PRETTY_NAME:-not Ubuntu}; it is for Ubuntu 24.04 or 26.04"

# RELEASES: what differs between them, and nothing else does.
#   REPO          NVIDIA's repository for the release
#   KEYRING_SHA   its cuda-keyring_1.1-1_all.deb, pinned at first use over
#                 HTTPS (24.04's on 27 October 2026, 26.04's on 29 October)
#   MODULES       Ubuntu's prebuilt, signed module package, after the branch
#   DRIVER, CUDA  the release's defaults
#   NEWEST_ONLY   1: the release has no toolkit for a card before Turing
case "${VERSION_ID:-}" in
    24.04) REPO=ubuntu2404; KEYRING_SHA=d2a6b11c096396d868758b86dab1823b25e14d70333f1dfa74da5ddaf6a06dba
           MODULES=generic-hwe-24.04; DEF_DRIVER=580-server; DEF_CUDA=12-9; NEWEST_ONLY=0 ;;
    26.04) REPO=ubuntu2604; KEYRING_SHA=f7f474b5f6a4adf987aa587920df00e713285958ef6a913dda1945a544a3099e
           MODULES=generic; DEF_DRIVER=595-server; DEF_CUDA=13-4; NEWEST_ONLY=1 ;;
    *) die "this is Ubuntu ${VERSION_ID:-?}; it knows 24.04 (any card) and 26.04 (Turing or newer)" ;;
esac
DRIVER="${DRIVER:-$DEF_DRIVER}"
CUDA="${CUDA:-$DEF_CUDA}"
CUDA_DOT="${CUDA/-/.}"
KEYRING_URL=https://developer.download.nvidia.com/compute/cuda/repos/$REPO/x86_64/cuda-keyring_1.1-1_all.deb

_gpu=$(lspci -nn | grep -iE 'vga|3d controller' | grep -i nvidia || true)
[ -n "$_gpu" ] || die "no NVIDIA GPU on the PCI bus"
note "GPU: ${_gpu#*: }"
# The card's generation from its PCI device id, before any driver: NVIDIA
# numbers its chips in order, and Turing begins at 10de:1e00 (TU102). Below
# it are Maxwell, Pascal and Volta, compute under 7.5, which CUDA 13 no
# longer builds for.
_id=$(printf '%s\n' "$_gpu" | grep -o '\[10de:[0-9a-f]\{4\}\]' | head -n1 | cut -c7-10)
OLD_CARD=0
if [ -n "$_id" ] && [ $((16#$_id)) -lt $((16#1e00)) ]; then OLD_CARD=1; fi
if [ "$OLD_CARD" = 1 ]; then
    note "card 10de:$_id: before Turing (Maxwell, Pascal or Volta): driver 580 is its last, CUDA 12 its last toolkit"
    [ "$NEWEST_ONLY" = 0 ] || die "Ubuntu $VERSION_ID has only CUDA 13, which does not build for this card: install Ubuntu 24.04"
    case "$CUDA" in 1[3-9]-*) die "CUDA $CUDA_DOT does not build for this card: CUDA=12-9" ;; esac
else
    note "card 10de:${_id:-?}: Turing or newer: CUDA $CUDA_DOT"
fi

# ---- 2. the driver ---------------------------------------------------------
say "The driver: Ubuntu's nvidia ${DRIVER} branch"
_installed=$(dpkg-query -W -f='${Package}\n' 'nvidia-kernel-common-*' 2>/dev/null | sed 's/^nvidia-kernel-common-//' | head -n1 || true)
if [ -n "$_installed" ] && [ "$_installed" != "$DRIVER" ]; then
    note "installed branch: $_installed (asked for $DRIVER); keeping what is there"
    DRIVER=$_installed
fi
case "$DRIVER" in
    580*) ;;
    *) [ "$OLD_CARD" = 0 ] || die "branch $DRIVER: this card's last branch is 580 (DRIVER=580-server)" ;;
esac
_pkgs="nvidia-utils-$DRIVER"
if [ -z "$_installed" ]; then
    _pkgs="nvidia-headless-no-dkms-$DRIVER linux-modules-nvidia-$DRIVER-$MODULES $_pkgs"
    note "no NVIDIA driver installed: adding the headless $DRIVER branch with Ubuntu's prebuilt modules"
fi
apt-get update -qq
# shellcheck disable=SC2086  # a list of package names
apt-get install -y -qq $_pkgs
note "driver packages: $(dpkg-query -W -f='${Package} ${Version}\n' "nvidia-utils-$DRIVER" "nvidia-kernel-common-$DRIVER" 2>/dev/null | tr '\n' ';')"

# ---- 3. NVIDIA's CUDA repository, kept low -------------------------------
say "NVIDIA's CUDA repository (keyring pinned, priority 100)"
if ! dpkg-query -W cuda-keyring >/dev/null 2>&1; then
    _k=$(mktemp --suffix=.deb)
    curl -fsSL -o "$_k" "$KEYRING_URL" || die "could not download $KEYRING_URL"
    echo "$KEYRING_SHA  $_k" | sha256sum -c --quiet - || die "cuda-keyring did not match its pinned sha256 -- not installing it"
    dpkg -i "$_k" >/dev/null
    rm -f "$_k"
    note "cuda-keyring installed"
else
    note "cuda-keyring already installed"
fi
# Ubuntu's archive is 500. At 100, a package NVIDIA's repository shares with
# Ubuntu (a driver, a libnvidia-*) is never chosen over Ubuntu's, and nothing
# from it is installed unless named; a package only it has (cuda-toolkit-*)
# installs when asked for, and upgrades within itself.
cat > /etc/apt/preferences.d/cuda-repository-low <<'PIN'
# Written by orrery's tools/cuda-node.sh: NVIDIA's CUDA repository below
# Ubuntu's archive, so its driver packages never replace Ubuntu's own branch.
Package: *
Pin: origin developer.download.nvidia.com
Pin-Priority: 100
PIN
apt-get update -qq

# ---- 4. the toolkit --------------------------------------------------------
say "CUDA toolkit $CUDA_DOT: about 4 GB to download, then unpacked (minutes)"
# Not -qq: the first run was silent here for five minutes at 7 MB/s and
# looked stopped. apt's own lines say what it is fetching.
apt-get install -y "cuda-toolkit-$CUDA"
[ -x "/usr/local/cuda-$CUDA_DOT/bin/nvcc" ] || die "no nvcc at /usr/local/cuda-$CUDA_DOT/bin after installing cuda-toolkit-$CUDA"

# ---- 5. paths --------------------------------------------------------------
cat > /etc/profile.d/cuda.sh <<EOF
# Written by orrery's tools/cuda-node.sh.
case ":\$PATH:" in *":/usr/local/cuda-$CUDA_DOT/bin:"*) ;; *) PATH="/usr/local/cuda-$CUDA_DOT/bin:\$PATH" ;; esac
export PATH
EOF
echo "/usr/local/cuda-$CUDA_DOT/lib64" > /etc/ld.so.conf.d/cuda-orrery.conf
ldconfig
note "PATH and library path set for every login (/etc/profile.d/cuda.sh)"

# ---- 6. the check ------------------------------------------------------------
say "Check: nvidia-smi"
# A driver installed by this run is not loaded yet: load it, and if the card
# is held (nouveau) say so and go on to the fleet's tools; the check runs on
# the next run, after a reboot. calber and boburner came with the driver;
# a machine without one comes here.
_loaded=1
if ! nvidia-smi >/dev/null 2>&1; then
    modprobe nvidia 2>/dev/null || true
    nvidia-smi >/dev/null 2>&1 || _loaded=0
fi
if [ "$_loaded" = 0 ]; then
    note "the driver is installed but not running (a new install, or nouveau holds the card): REBOOT, then run this again for the check"
else
nvidia-smi --query-gpu=name,driver_version,memory.total,temperature.gpu --format=csv

say "Check: a CUDA program, compiled for this card and run"
_d=$(mktemp -d)
cat > "$_d/check.cu" <<'CU'
#include <cstdio>
#include <cuda_runtime.h>
__global__ void add(const float *a, const float *b, float *c, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] = a[i] + b[i];
}
int main() {
    cudaDeviceProp p;
    if (cudaGetDeviceProperties(&p, 0) != cudaSuccess) { printf("no CUDA device\n"); return 1; }
    printf("device: %s, compute %d.%d, %d SMs, %.1f GB\n", p.name, p.major, p.minor,
           p.multiProcessorCount, p.totalGlobalMem / 1073741824.0);
    const int n = 1 << 20;
    float *a, *b, *c;
    cudaMallocManaged(&a, n * sizeof(float));
    cudaMallocManaged(&b, n * sizeof(float));
    cudaMallocManaged(&c, n * sizeof(float));
    for (int i = 0; i < n; i++) { a[i] = i; b[i] = 2 * i; }
    add<<<(n + 255) / 256, 256>>>(a, b, c, n);
    if (cudaDeviceSynchronize() != cudaSuccess) { printf("kernel failed: %s\n", cudaGetErrorString(cudaGetLastError())); return 1; }
    int bad = 0;
    for (int i = 0; i < n; i++) if (c[i] != 3.0f * i) bad++;
    printf("vector add of %d floats: %s\n", n, bad ? "WRONG" : "correct");
    return bad ? 1 : 0;
}
CU
"/usr/local/cuda-$CUDA_DOT/bin/nvcc" -arch=native -o "$_d/check" "$_d/check.cu"
"$_d/check"
rm -rf "$_d"
fi

# ---- 7. the fleet's tools: tmux and copal-tm ---------------------------------
say "The fleet's tools: tmux, and copal-tm for ${SUDO_USER:-root}"
dpkg-query -W tmux >/dev/null 2>&1 || apt-get install -y -qq tmux
note "tmux $(tmux -V 2>/dev/null | cut -d' ' -f2)"
# copal-tm is built by the user who will run it, with that user's cargo
# (rustup), never as root: the build and the link are in their home.
_u="${SUDO_USER:-}"
if [ -n "$_u" ] && [ "$_u" != root ]; then
    if sudo -u "$_u" -H bash -c '[ -f ~/.cargo/env ] && . ~/.cargo/env; command -v cargo' >/dev/null 2>&1; then
        sudo -u "$_u" -H bash -c 'set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env
            mkdir -p ~/code ~/.local/bin
            if [ -d ~/code/copal-tm/.git ]; then git -C ~/code/copal-tm pull -q --ff-only || true
            else git clone -q https://github.com/vonglurt/copal-tm.git ~/code/copal-tm; fi
            cd ~/code/copal-tm && cargo build -q --release
            ln -sf ~/code/copal-tm/target/release/copal-tm ~/.local/bin/copal-tm'
        note "copal-tm: $(sudo -u "$_u" -H bash -c '~/.local/bin/copal-tm --help 2>&1 | head -1')"
    else
        note "copal-tm skipped: $_u has no cargo (install rustup for $_u, then run this again)"
    fi
else
    note "copal-tm skipped: run this with sudo from the user who will use it"
fi

say "Done: $(hostname) is a CUDA node (driver $DRIVER, CUDA $CUDA_DOT). Log: $LOG"
note "A new login has nvcc on PATH; this one: . /etc/profile.d/cuda.sh"
