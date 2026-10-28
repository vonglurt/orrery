# Held Back on Purpose: the Versions a GTX 1080 Ti Can Run CUDA On, and Nix as a Way to Pin Them

*Lab Report — IEEE Format · 28 October 2026 · interim: the Burn probe is building*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `docs/cuda-node-lab-report.md` (setting up calber);
`tools/cuda-node.sh`.

---

## Abstract

A GTX 1080 Ti is a Pascal card (compute capability 6.1), and NVIDIA is
retiring Pascal. Almost every version on the CUDA node calber was chosen
*older* than the newest, because the newest would not run CUDA on this card:
Ubuntu 24.04 rather than 26.04, driver series 580 rather than 590, CUDA 12.9
rather than 13. This report records each held-back version, the evidence
that forced it, how it is held so an update cannot undo it, and what will
end it. It then considers Nix (a flake, or `nix shell`) as a way to pin the
*build* dependencies (the Rust toolchain, the CUDA toolkit, the libraries a
GPU program links) per project and per commit, leaving the kernel driver to
the host, which is where it has to stay.

## I. Objective

1. Say, for each layer from the kernel to the training library, which
   version calber runs and why not the newer one.
2. Say how each is held, and what to watch.
3. Weigh Nix flakes and `nix shell` for the build dependencies.

## II. Why Pascal forces old versions

| Evidence | Where it came from |
| --- | --- |
| NVIDIA's **580** driver series is the last for Maxwell, Pascal and Volta; **590** drops them | NVIDIA's deprecation notice, reported by Phoronix and others (July 2025) |
| **CUDA 12.x** is the last toolkit for those architectures; CUDA 13 does not target them | the same notice; and on calber `nvcc` 12.9 itself warns: "Support for offline compilation for architectures prior to `sm_75` will be removed in a future release" |
| NVIDIA's apt repository for **Ubuntu 26.04** has `cuda-toolkit-13-3` and `13-4` only, and no driver packages | read from `developer.download.nvidia.com/compute/cuda/repos/ubuntu2604/x86_64/`, 27 October 2026 |
| NVIDIA's repository for **Ubuntu 24.04** has drivers 550 to **580** and `cuda-toolkit-12-5` to **`12-9`** (and 13.x) | `.../ubuntu2404/x86_64/`, the same day |
| Arch moved Pascal off its default NVIDIA packages with 590 | reported when calber still ran Omarchy, which is part of why it was replaced |

## III. The versions, layer by layer

| Layer | On calber | Newer one not taken | Why | Held by |
| --- | --- | --- | --- | --- |
| OS | **Ubuntu 24.04.5 LTS** | 26.04.1 LTS (offered at login: "New release '26.04.1 LTS' available") | NVIDIA's CUDA 12 packages exist for 24.04, not 26.04 | not upgrading (see VI: the prompt should be turned off) |
| Kernel | 7.0.0-38-generic (24.04's HWE kernel) | — | the newest 24.04 offers; Ubuntu builds and signs the NVIDIA module for each of its kernels | Ubuntu's `linux-modules-nvidia-580-server-generic-hwe-24.04`, which follows the kernel |
| Driver | **580.178.04** (`580-server` branch, Ubuntu's) | 590 and later | 590 drops Pascal | a branch package: `580-server` only ever upgrades within 580; NVIDIA's repository pinned to priority 100 so its drivers never replace Ubuntu's |
| CUDA toolkit | **12.9.2** (`nvcc` 12.9.86) | 13.0 to 13.4 | CUDA 13 does not target Pascal | the versioned package `cuda-toolkit-12-9`, never the unversioned `cuda` or `cuda-toolkit` metapackages |
| Compute capability | `sm_61` | — | the card's | `nvcc -arch=native` |
| Rust | 1.99.1 (rustup, user's) | — | current; nothing about Pascal holds it | rustup, per user |
| Training library | **Burn 0.22.1**, CubeCL 0.11.3, CUDA backend | — | the newest; **whether it still runs on compute 6.1 is what the probe is testing** | BURN_RESULT |

`ubuntu-drivers` recommends `nvidia-driver-470` for the card. That is an
*older* series and was not taken: 580 is newer, still supports Pascal, and is
what CUDA 12.9 needs.

## IV. What ends each hold

- **Driver 580:** security updates for a retired series stop at some point
  NVIDIA announces; after that the card is a security question, not a CUDA
  one. Watch NVIDIA's driver support notes.
- **Ubuntu 24.04:** standard support to April 2029.
- **CUDA 12.9:** nothing ends it on this card; it ends the *software*: a
  library that requires CUDA 13 (a newer Burn, PyTorch build or cuDNN) will
  not run here. Each such library is a version to hold too.
- **The card itself:** a Turing or newer card (compute 7.5 and up) lifts
  every hold in this report.

## V. Burn on Pascal: the probe

Burn's CUDA backend does not ship precompiled kernels; CubeCL generates
them at run time and compiles them with NVIDIA's runtime compiler (NVRTC),
for the card in front of it. Whether that path still accepts `sm_61` is not
something the documentation settles, so it is measured: a Rust program on
calber (`~/code/burnprobe`), Burn 0.22.1 with `cuda` and `autodiff`:

1. a 256x256 matrix product on the GPU, compared with the same product
   done on the CPU (ok if the largest difference is under 1e-3);
2. 200 Adam steps on a net of the learned tank's shape (143 -> 64 -> 64 ->
   8) fitting a fixed random target (learning if the loss falls by more than
   a fifth).

Result: BURN_RESULT_DETAIL

If it fails, the alternatives, in order: Burn's LibTorch backend (`tch`)
with a PyTorch build that still includes `sm_61`; an older Burn and CubeCL
pinned to the last versions that did; or a newer card.

## VI. Recommendations

1. **Turn off the release upgrade prompt** on calber (`Prompt=never` in
   `/etc/update-manager/release-upgrades`), so 26.04 is not taken by
   accident; `tools/cuda-node.sh` should do it for a Pascal card.
2. **Never install the unversioned `cuda` or `cuda-toolkit` metapackages** on
   it: they would pull CUDA 13.
3. **Record the version of everything a GPU program links** (Burn, CubeCL,
   the CUDA libraries) beside its results, as the census records its binary.

## VII. Nix: pinning the build dependencies

### What it would pin, and what it cannot

| | Nix can pin it | Why |
| --- | --- | --- |
| Rust toolchain, `cargo`, `pkg-config`, C libraries | yes | ordinary packages |
| CUDA toolkit pieces (`nvcc`, `cudart`, `nvrtc`, cuBLAS) | yes | nixpkgs packages NVIDIA's redistributable CUDA by version (a `cudaPackages_12_x` set); unfree, so `allowUnfree` and `cudaSupport` must be set |
| The kernel driver and `libcuda.so` | **no, on Ubuntu** | the user-space driver must match the loaded kernel module exactly; it comes from the host. A Nix shell on a non-NixOS host is pointed at the host's driver libraries (a wrapper such as nixGL, or the host's `libcuda` on `LD_LIBRARY_PATH`) |

### Flake or `nix shell`

| | `nix shell` / `nix-shell -p ...` | a flake (`flake.nix` + `flake.lock`) with a `devShell` |
| --- | --- | --- |
| What it is | a shell with the named packages, for now | the project's dependencies, declared in the repository |
| Pinned | to whatever nixpkgs the machine has | to one nixpkgs commit, in `flake.lock`, in git |
| Same on every machine | no | yes: `nix develop` on calber, the Mac, a fleet node gives the same toolchain |
| Good for | trying something once | treadcore's GPU crate, so a result can be rebuilt with the exact toolchain that made it |

### Why it fits this project

- **It is the census's own rule, for toolchains.** treadcore freezes the
  census binary because a different build plays different matches; a
  `flake.lock` freezes the toolchain the same way, and the commit that made
  a result names it.
- **It holds old versions after the world moves on.** nixpkgs will drop
  CUDA 12 in time; a locked flake keeps the commit that had it, and that is
  exactly the Pascal problem.
- **It does not touch apt.** The driver stays Ubuntu's (held as in III); the
  build world lives in `/nix/store` beside it, and can be removed.
- **On Copal too, for build dependencies.** Nix runs on Alpine (musl) and
  can provide glibc toolchains there; not CUDA, since a Copal node has no
  NVIDIA driver.

### Costs

- Nix's store is large, and a first `nix develop` with CUDA downloads
  gigabytes.
- Unfree CUDA packages are not in the public binary cache's free set, so
  they are fetched from NVIDIA's redistributables; the attribute names and
  which CUDA versions a nixpkgs commit carries must be checked at the pinned
  commit, not assumed.
- Another tool to learn and keep; and the driver half stays outside it,
  which is where most of the CUDA trouble lives.

### Proposed experiment

A `flake.nix` for treadcore's GPU crate: Rust (pinned), `cudaPackages_12_x`
with `nvcc` and `nvrtc`, `pkg-config`; the host's driver libraries passed
in; `nix develop` on calber builds the same Burn program as the probe, and
the probe's result must match. If it does, the GPU work is built that way
from then on.

## VIII. Open items

1. The Burn probe's result (V).
2. The release upgrade prompt off on calber, and in `cuda-node.sh`.
3. The Nix experiment (VII).
