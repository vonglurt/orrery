# One Source for Two CUDAs: crucible on Pascal (CUDA 12.9) and Ada (CUDA 13), Compiled in Docker on an ARM Mac

*Lab Report — IEEE Format · 29 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `docs/cuda-node-lab-report.md` (the first CUDA node, calber, on
Ubuntu 24.04 with CUDA 12.9); crucible's `Makefile` and `src/crucible.cu`;
`tools/cuda-node.sh`.

---

## Abstract

saskatchewan, the fleet's census machine, runs Copal (Alpine, musl), and its
NVIDIA card has never been used. The card turned out to be an RTX 4060 Ti
16 GB (`10de:2805`, Ada Lovelace, compute 8.9), two generations past the
fleet's Pascal cards. Paul asked whether, on Ubuntu 26.04, the fleet's CUDA
program could run "without much translation", and whether one source could
serve both kinds of card, "only differing by a small DEFINE statement or
like that. Some machine specific aware."

It can, and it needs no DEFINE at all:

- **The source:** crucible uses 26 CUDA runtime and Thrust names. CUDA 13
  removed exactly one of them: the field `cudaDeviceProp::clockRate`, read
  once, for a line of print. It is replaced by
  `cudaDeviceGetAttribute(cudaDevAttrClockRate)`, which exists in CUDA 12
  and 13 alike.
- **The Makefile:** it now reads the card's compute capability from
  `nvidia-smi` and builds for it. It refuses, with a sentence, the one
  pairing that cannot work: CUDA 13 on a card older than Turing.
- **`tools/cuda-node.sh`:** it now holds Ubuntu 24.04 and 26.04 in one
  table, and reads the card's generation from its PCI id before any driver
  is installed.

With no CUDA 13 machine in the fleet, the CUDA 13 build was proved in
NVIDIA's own `linux/arm64` development container on this Mac (Apple
silicon, no NVIDIA GPU). Device code is compiled for the architecture you
name, not the one you run on, so a Mac can compile for an Ada card:
- the old source failed there, with the one predicted error, at line 451;
- the new source built for `sm_89`;
- the Makefile's guard refused `sm_61` under CUDA 13.

On calber, the new source built and ran on CUDA 12.9 as before.

## I. Objective

1. Find what in crucible depends on the CUDA version and the card.
2. Make one source and one Makefile build correctly on every card in the
   fleet, with nothing set by hand.
3. Prepare `cuda-node.sh` for Ubuntu 26.04 on saskatchewan.
4. Prove the CUDA 13 build without a CUDA 13 machine.

## II. Materials

| Item | Detail |
| --- | --- |
| The new card | saskatchewan, 192.168.8.235: i7-13700F (24 threads), 63 GB; PCI `0000:01:00.0` is `10de:2805`, **GeForce RTX 4060 Ti 16 GB** (AD106, Ada, compute 8.9). Read from `/sys/bus/pci/devices/0000:01:00.0/{vendor,device}`, since Copal has no `lspci` names and no NVIDIA driver |
| The fleet's cards | calber GTX 1080 Ti, boburner GTX 1070 Ti, whitewolf (Pascal, compute 6.1, `nvidia-smi --query-gpu=compute_cap` says `6.1`); Ubuntu 24.04, driver 580, CUDA 12.9 |
| The program | crucible, `src/crucible.cu` (696 lines): a self-learning loop entirely on the card, run by the fleet as island segments. The planner sends `src/crucible.cu` and `Makefile` with every segment and runs `make` on the node |
| The Mac | Apple silicon (`arm64`), colima's Docker, no NVIDIA GPU |
| The container | `nvidia/cuda:13.0.1-devel-ubuntu24.04`, `--platform linux/arm64`: `Cuda compilation tools, release 13.0, V13.0.88` |
| Ubuntu 26.04 | `resolute`, read from `archive.ubuntu.com/ubuntu/dists/resolute{,-updates}/restricted/binary-amd64/Packages.xz` (packages.ubuntu.com answered HTTP 500) |
| NVIDIA's repository | `developer.download.nvidia.com/compute/cuda/repos/ubuntu2604/x86_64/`: `cuda-toolkit-13-3` and `13-4` (13.4.3 the newest), driver pinning packages for 610 and 615, no 12.x |

## III. What crucible asks of CUDA

Every CUDA and Thrust name in the source, counted with `grep -o`:

| Kind | Names (uses) | In CUDA 13 |
| --- | --- | --- |
| Memory | `cudaMalloc` (8), `cudaMallocManaged` (1), `cudaMemcpy` (11) with `HostToDevice`, `DeviceToHost`, `DeviceToDevice`, `cudaMemset` (2), `cudaMemGetInfo` (1) | unchanged |
| Timing | `cudaEventCreate`, `cudaEventRecord`, `cudaEventSynchronize`, `cudaEventElapsedTime` | unchanged |
| Errors and sync | `cudaGetLastError`, `cudaGetErrorString`, `cudaDeviceSynchronize`, `cudaSuccess`, `cudaError` | unchanged |
| Occupancy | `cudaFuncGetAttributes`, `cudaOccupancyMaxActiveBlocksPerMultiprocessor` | unchanged |
| Device query | `cudaGetDeviceProperties`, reading `name`, `major`, `minor`, `multiProcessorCount`, `maxThreadsPerMultiProcessor`, **`clockRate`** | **`clockRate` removed in 13.0**; the rest unchanged |
| In kernels | `atomicAdd` (9), `__syncthreads` (2), `__launch_bounds__(256, MINB)`, shared memory for two nets | unchanged |
| Thrust | `device_ptr`, `sequence`, `sort_by_key` with `greater<float>`, `reduce` | unchanged (CCCL 3; built under `-std=c++17`) |
| Build | `-gencode arch=compute_61,code=sm_61` in the Makefile | **CUDA 13 does not build for compute under 7.5** |

So two things tie crucible to a CUDA and a card: one field, and one line of
the Makefile. Neither is in the simulation, the net or the training.

## IV. The hybrid: the machine says, the source does not branch

**Why there is no DEFINE.** A `#if CUDART_VERSION >= 13000` around the print
would have worked. But the replacement call exists in both toolkits, so a
branch would only keep the removed field alive on 12.9, for nothing. The
one thing that truly differs by machine is the architecture to compile for.
That belongs to the build, and the card can be asked for it.

**The source** (`src/crucible.cu`, at the device line):

```c
// The clock as an attribute, not cudaDeviceProp::clockRate: CUDA 13
// removed the field, and the attribute is in 12 and 13 alike, so one
// source builds on the Pascal cards (12.9) and the Ada one (13).
int khz = 0;
CK(cudaDeviceGetAttribute(&khz, cudaDevAttrClockRate, 0));
```

**The Makefile**, in three parts:

| Part | What | Why |
| --- | --- | --- |
| `CAP` | `nvidia-smi --query-gpu=compute_cap` of the first card, `6.1` read as `61`, asked once (`ifndef`, `:=`). `61` if nvidia-smi cannot say; `make CAP=89` names one | the driver's own answer (there since driver 510), so a new card needs no edit |
| `ARCH` | `-gencode arch=compute_$(CAP),code=sm_$(CAP)` | the same form as before, so a Pascal node builds what it built |
| the guard | `nvcc --version`'s major. 13 or more with `CAP` under 75 stops with "crucible: CUDA 13 cannot build for sm_61: keep CUDA 12 on this card" | a sentence in the segment's log, not nvcc's unsupported-architecture error |

`make which` prints what this machine would build: `card sm_61, CUDA 12
(/usr/local/cuda/bin/nvcc), H=32, MINB=2`.

**Why not `nvcc -arch=native`?** It also builds for the card present, and
`cuda-node.sh`'s check program uses it. But it says nothing about what it
chose, and it cannot be checked against the toolkit before nvcc runs. The
explicit `CAP` can be printed, overridden and guarded.

**The node script** (`tools/cuda-node.sh`). What differs between the two
releases is one table, and nothing else in the script branches on the
release:

| | 24.04 | 26.04 |
| --- | --- | --- |
| NVIDIA's repository | `ubuntu2404` | `ubuntu2604` |
| keyring sha256 (first use, over HTTPS) | `d2a6b11c…6dba` (27 Oct; it still matched today) | `f7f474b5…099e` (29 Oct) |
| Ubuntu's module package | `linux-modules-nvidia-<branch>-generic-hwe-24.04` | `linux-modules-nvidia-<branch>-generic` |
| default driver | `580-server` | `595-server` (the newest headless server branch in `resolute`; 580 to 595 and 610 are there) |
| default CUDA | `12-9` | `13-4` |
| cards | any | Turing or newer only |

**The card is read before there is a driver.** NVIDIA numbers its chips in
order, and Turing begins at `10de:1e00` (TU102). A device id below that is
Maxwell, Pascal or Volta, with compute under 7.5. On 26.04 such a card
stops the run before anything is installed: "Ubuntu 26.04 has only CUDA 13,
which does not build for this card: install Ubuntu 24.04". On 24.04 it is
held to driver 580 and CUDA 12.

## V. Results

| Test | Where | Result |
| --- | --- | --- |
| New source, card's own arch | calber, CUDA 12.9 | `make which`: `card sm_61, CUDA 12`; built (128 registers in `play`, as before) |
| New source, run | calber, one generation | `# device NVIDIA GeForce GTX 1080 Ti, sm_61, 28 SMs, 1683 MHz, 11.7 GB (11.2 free)`: the clock through the attribute |
| New source for Ada on 12.9 | calber, `make CAP=89` | built |
| Guard (a fake nvcc saying release 13.0) | calber, sm_61 | stopped at the guard (`Makefile:27`) |
| **Old source on CUDA 13** | Mac, container, `sm_89` | `src/crucible.cu(451): error: class "cudaDeviceProp" has no member "clockRate"`, `1 error detected`: the one predicted, and no other |
| **New source on CUDA 13** | Mac, container, `make CAP=89` | `card sm_89, CUDA 13`; built, 3,059,040 bytes |
| Guard on CUDA 13 | Mac, container, `make CAP=61` | stopped (`Makefile:27`) |
| Card generation from PCI id | Mac, the script's own lines | `1b06` (GTX 1080 Ti): before Turing; `2805` (RTX 4060 Ti): Turing or newer; `2184` (GTX 1660, Turing): Turing or newer |
| 24.04 keyring | NVIDIA, today | still `d2a6b11c…6dba`, as pinned |
| `cuda-node.sh` | Mac | `bash -n` clean |

## VI. Discussion

**Compiling for a GPU does not need a GPU.** `nvcc` turns device code into
code for the architecture it is given (`sm_89` here). The host CPU only
decides what the host half of the binary runs on. So NVIDIA's `arm64`
development image, on a Mac with no NVIDIA hardware at all, is a full CUDA
13 compiler for an Ada card. The container's warning, "The NVIDIA Driver
was not detected", is true and does not matter for a build. This turned the
question "will it compile on 13?" from install day's problem into a minute's
work today, and it showed that the predicted single error was the only one.
Its limits:
- **It proves the build, not the run.** The kernels' behaviour on Ada,
  driver 595 and CUDA 13.4 is for the first segment on saskatchewan.
- **The versions differ.** The container was CUDA 13.0.1 and saskatchewan
  will get 13.4. The removal of `clockRate` was in 13.0, so a later 13
  removes nothing crucible needs unless its notes say so.
- **colima shares only the home folder.** The first attempt mounted the
  session's scratch folder under `/private/tmp` and the container saw
  nothing ("No such file or directory"). The copy went under `~/.cache`.

**Ask the card what it is, not the machine its name.** The Makefile asks the
driver for the compute capability, and the node script reads the PCI id.
Neither names a machine. A card added next year, or saskatchewan's if it
moves, needs no edit. The fleet-wide effect comes free, because the planner
already sends the Makefile with every segment.

**The one pairing that cannot work stops with a sentence.** CUDA 13 on
Pascal fails inside nvcc with an architecture error that names neither the
card nor the fix. The guard says both, once in the Makefile and once,
earlier, in the node script. That is the same lesson as the first CUDA
report: say what is happening.

**What the Ada card brings.** 16 GB, and more shared memory and registers
per SM than Pascal. A wider net (`make H=48`) fits as it does on Pascal.
Whether anything larger is worth it is a question for crucible's own
numbers.

## VII. Install day, saskatchewan

1. Install Ubuntu 26.04 Server with "third-party" ticked, from Copal's boot
   menu or the ISO.
2. From the Mac: `scp tools/cuda-node.sh user@saskatchewan: && ssh -t
   user@saskatchewan sudo bash cuda-node.sh`. It reads the card, keeps or
   installs 595-server, installs CUDA 13.4, then compiles and runs its
   check program. It says if it needs a reboot.
3. Add `saskatchewan-cuda` to the queue's hosts (caps `cuda`) beside
   `saskatchewan` (caps `cpu`), as calber and boburner have.
4. The first crucible segment builds itself (`card sm_89, CUDA 13`). Its log
   is the first run of crucible on Ada.

While it is down, saskatchewan's CPU lane, the largest in the fleet, waits:
the keeper marks it unreachable, and its running job goes back on the queue.

## References

1. NVIDIA, "CUDA Features Archive", CUDA 13.0: deprecated fields removed
   from `cudaDeviceProp`, `clockRate` → `cudaDeviceGetAttribute(cudaDevAttrClockRate)`.
   https://docs.nvidia.com/cuda/cuda-features-archive/index.html
2. NVIDIA, "CUDA Toolkit 13.0 Release Notes".
   https://docs.nvidia.com/cuda/archive/13.0.0/cuda-toolkit-release-notes/
3. Eigen issue 3000, "CUDA 13 compatibility": `clockRate` and `computeMode`
   breaking a build. https://gitlab.com/libeigen/eigen/-/issues/3000
4. orrery, `docs/cuda-node-lab-report.md`: the 24.04, 580, CUDA 12.9 choice
   for Pascal.
