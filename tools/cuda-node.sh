#!/bin/bash
# Make an Ubuntu 24.04 machine with an NVIDIA GPU a CUDA node of the fleet.
#
# Run it ON THE NODE, as root. From the console's machine:
#
#   scp tools/cuda-node.sh user@NODE:  &&  ssh -t user@NODE sudo bash cuda-node.sh
#
# Safe to run again: every step checks before it acts. Everything it prints
# also goes to /var/log/cuda-node.log.
#
# WHY UBUNTU, AND WHY 24.04. CUDA needs NVIDIA's own driver, whose user space
# is built for glibc; a Copal node is Alpine (musl), so a CUDA machine runs
# Ubuntu and joins the fleet over SSH. For a GTX 900 or 10 series card
# (Maxwell, Pascal) the 580 driver series is the last, and CUDA 12 the last
# toolkit; NVIDIA's repository for Ubuntu 26.04 has only CUDA 13, its 24.04 one
# driver 580 and CUDA 12.9. So: 24.04 (Copal's boot menu has "Install Ubuntu
# 24.04 LTS Server (network): for NVIDIA Pascal + CUDA 12").
#
# WHAT IT DOES, in order:
#   1. checks: root, Ubuntu 24.04, an NVIDIA GPU
#   2. the driver: keeps Ubuntu's 580 branch if installed (the installer's
#      "third-party" choice puts in 580-server, with Ubuntu's prebuilt signed
#      modules), or installs it; adds nvidia-utils for nvidia-smi
#   3. NVIDIA's CUDA repository, by its keyring package (sha256 pinned), at a
#      LOW apt priority, so none of its driver packages can ever replace
#      Ubuntu's: from it only what is asked for by name is installed
#   4. the CUDA toolkit (12.9 by default; CUDA=13-0 for a Turing or newer card)
#   5. PATH and the library path for every login
#   6. the check: nvidia-smi, then a small CUDA program compiled for this
#      card and run: device properties and a million-element vector add
#
# Settings: DRIVER=580-server (the Ubuntu branch), CUDA=12-9 (the toolkit).
set -euo pipefail

DRIVER="${DRIVER:-580-server}"
CUDA="${CUDA:-12-9}"
CUDA_DOT="${CUDA/-/.}"
KEYRING_URL=https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2404/x86_64/cuda-keyring_1.1-1_all.deb
# First use: what the URL served on 27 October 2026, over HTTPS.
KEYRING_SHA=d2a6b11c096396d868758b86dab1823b25e14d70333f1dfa74da5ddaf6a06dba

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
[ "${ID:-}" = ubuntu ] || die "this is ${PRETTY_NAME:-not Ubuntu}; it is for Ubuntu 24.04"
[ "${VERSION_ID:-}" = 24.04 ] || die "this is Ubuntu ${VERSION_ID:-?}; NVIDIA's CUDA 12 repository is for 24.04 (26.04 has CUDA 13 only)"
_gpu=$(lspci -nn | grep -iE 'vga|3d controller' | grep -i nvidia || true)
[ -n "$_gpu" ] || die "no NVIDIA GPU on the PCI bus"
note "GPU: ${_gpu#*: }"

# ---- 2. the driver ---------------------------------------------------------
say "The driver: Ubuntu's nvidia ${DRIVER} branch"
_installed=$(dpkg-query -W -f='${Package}\n' 'nvidia-kernel-common-*' 2>/dev/null | sed 's/^nvidia-kernel-common-//' | head -n1 || true)
if [ -n "$_installed" ] && [ "$_installed" != "$DRIVER" ]; then
    note "installed branch: $_installed (asked for $DRIVER); keeping what is there"
    DRIVER=$_installed
fi
case "$DRIVER" in
    580*) ;;
    *) note "branch $DRIVER: a GTX 900 or 10 series card needs 580; newer cards are fine on it too" ;;
esac
_pkgs="nvidia-utils-$DRIVER"
if [ -z "$_installed" ]; then
    _pkgs="nvidia-headless-no-dkms-$DRIVER linux-modules-nvidia-$DRIVER-generic-hwe-24.04 $_pkgs"
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
# Ubuntu's archive, so its driver packages never replace Ubuntu's 580 branch.
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

say "Done: $(hostname) is a CUDA node (driver $DRIVER, CUDA $CUDA_DOT). Log: $LOG"
note "A new login has nvcc on PATH; this one: . /etc/profile.d/cuda.sh"
