# A CUDA Node for the Fleet: a GTX 1080 Ti on Ubuntu 24.04, Set Up over SSH

*Lab Report — IEEE Format · 27–28 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: Copal's `docs/legacy-bios-lab-report.md` ("Ubuntu from the boot
menu, for a CUDA machine"); `tools/cuda-node.sh`, the script this records.

---

## Abstract

A GTX 1080 Ti machine was taken from Omarchy (Arch) to Ubuntu 24.04 LTS,
installed over the network from a Copal stick's boot menu, and made a CUDA
node with one script, `tools/cuda-node.sh`, run over SSH from the Mac. The
choices follow from the card. CUDA needs NVIDIA's own driver, whose user
space is glibc-only, so not Copal (Alpine, musl). The 1080 Ti is Pascal:
the 580 driver series is its last, and CUDA 12 its last toolkit. NVIDIA's
CUDA repository for Ubuntu 26.04 carries only CUDA 13, so the release is
24.04. Ubuntu's installer, with "third-party" ticked, had already put in
the 580-server driver with Ubuntu's prebuilt, signed kernel modules. The
script keeps it, adds `nvidia-smi`, adds CUDA 12.9 from NVIDIA's repository
at a priority too low for any of its driver packages to replace Ubuntu's,
and ends with a CUDA program compiled for the card and run: on the first
run, the 1080 Ti reported compute 6.1, 28 SMs and 10.9 GB, and added a
million floats correctly. That run looked stopped for five minutes while it
downloaded at 7.7 MB/s with its progress hidden; the script now shows it.

## I. Objective

1. Make the 1080 Ti machine run CUDA, on a setup NVIDIA supports for
   Pascal.
2. Do it from the fleet's console machine over SSH, with one script that can
   be run again, on this node and the next.
3. Record why each choice was made, for the next card.

## II. Materials

| Item | Detail |
| --- | --- |
| Node | `calber`, 192.168.8.181: Intel Core i7-9700F (8 cores), 62 GB, 1.8 TB NVMe; NVIDIA GeForce GTX 1080 Ti (GP102, `10de:1b06`, Gigabyte `1458:3752`); Secure Boot off |
| System | Ubuntu 24.04.5 LTS, kernel 7.0.0-38-generic (HWE), installed by "Install Ubuntu 24.04 LTS Server (network)" from a Copal stick written on saskatchewan; "third-party" ticked; OpenSSH server |
| Console | the iMac, `ssh user@calber` with Paul's key (`ssh-copy-id`) |
| Script | `tools/cuda-node.sh`, copied to the node and run as `sudo bash cuda-node.sh` |

## III. Why these choices

| Question | Answer | Because |
| --- | --- | --- |
| Why not Copal? | Ubuntu | NVIDIA's driver user space and CUDA are built for glibc; Copal is Alpine (musl). One attempt to repackage the driver for Alpine was abandoned for that reason |
| Which Ubuntu? | **24.04 LTS** | NVIDIA's repository for 26.04 has CUDA 13.3 and 13.4 only and no driver packages; for 24.04, drivers to 580 and CUDA 12.5 to 12.9 (read 27 October). CUDA 13 does not target Pascal |
| Which driver? | **Ubuntu's `580-server` branch**, kept | 580 is the last series for Maxwell, Pascal and Volta. The installer's third-party choice installed `nvidia-headless-no-dkms-580-server` with `linux-modules-nvidia-580-server-generic-hwe-24.04`: modules Ubuntu builds and signs for each kernel, so a kernel update needs no rebuild |
| Why not NVIDIA's `cuda-drivers-580`? | not installed | NVIDIA's packages build the module with DKMS on the node and can replace Ubuntu's; the prebuilt ones already work |
| How is NVIDIA's repository kept from replacing it? | apt priority 100 for `origin developer.download.nvidia.com` (`/etc/apt/preferences.d/cuda-repository-low`) | Ubuntu's archive is 500: a package both have comes from Ubuntu; a package only NVIDIA has (`cuda-toolkit-12-9`) installs when named. A future driver series without Pascal cannot arrive by `apt upgrade` |
| Which CUDA? | **12.9** (`cuda-toolkit-12-9`, 12.9.2) | the newest CUDA 12; settable (`CUDA=13-0` for a Turing or newer card) |
| `ubuntu-drivers` recommends `nvidia-driver-470`. Why not? | ignored | 470 is older; 580 is newer and still supports Pascal, and is what CUDA 12.9 needs |

## IV. The script, `tools/cuda-node.sh`

Run on the node as root; every step checks before it acts, so it can be run
again; everything it prints also goes to `/var/log/cuda-node.log`; a
failed command that ends it prints `STOPPED at line N` with the command, as
Copal's card build does.

1. **Checks:** root; `ID=ubuntu`, `VERSION_ID=24.04` (26.04 is refused,
   with the reason); an NVIDIA GPU on the PCI bus.
2. **Driver:** finds the installed branch (`nvidia-kernel-common-*`), keeps
   it, and adds `nvidia-utils-<branch>` for `nvidia-smi`; with no driver,
   installs the headless 580-server branch with Ubuntu's prebuilt modules.
3. **NVIDIA's repository:** `cuda-keyring_1.1-1_all.deb`, checked against a
   pinned sha256 (`d2a6b11c…6dba`, first use, 27 October) before `dpkg -i`;
   then the priority-100 pin.
4. **Toolkit:** `apt-get install -y cuda-toolkit-12-9`.
5. **Paths:** `/etc/profile.d/cuda.sh` puts `/usr/local/cuda-12.9/bin` on
   every login's PATH; `/etc/ld.so.conf.d/cuda-orrery.conf` and `ldconfig`
   for the libraries.
6. **The check:** `nvidia-smi` (name, driver, memory, temperature); then a
   CUDA program written, compiled with `nvcc -arch=native` (for this card)
   and run: the device's name, compute capability, SM count and memory, and
   a vector add of 2^20 floats in managed memory, every element checked.

## V. The run

From the Mac, after `ssh-copy-id user@192.168.8.181`. Paul ran it from a
login on calber itself (`ssh -t user@192.168.8.181 sudo bash cuda-node.sh`,
which connects calber to itself; it works, with one more host key and
password). From its output:

| Step | Result |
| --- | --- |
| checks | `GPU: NVIDIA Corporation GP102 [GeForce GTX 1080 Ti] [10de:1b06] (rev a1)` |
| driver | `nvidia-utils-580-server 580.178.04-0ubuntu0.24.04.1` installed beside `nvidia-kernel-common-580-server` of the same version |
| repository | `cuda-keyring installed` (sha256 matched) |
| toolkit | `cuda-toolkit-12-9` (12.9.2) installed; `/etc/profile.d/cuda.sh` and the library path written |
| `nvidia-smi` | `NVIDIA GeForce GTX 1080 Ti, 580.178.04, 11264 MiB, 36` (°C) |
| CUDA program | `device: NVIDIA GeForce GTX 1080 Ti, compute 6.1, 28 SMs, 10.9 GB`; `vector add of 1048576 floats: correct` |
| end | `Done: calber is a CUDA node (driver 580-server, CUDA 12.9)` |

`nvcc` warned while compiling: "Support for offline compilation for
architectures prior to `sm_75` will be removed in a future release". That is
NVIDIA's own statement of the boundary this report chose around: CUDA 12.9
still compiles for Pascal (`sm_61`), and the toolkit after it will not.

### What the toolkit brought with it

From the run's full output (Paul's terminal) and the node:

| | |
| --- | --- |
| time | started 00:30:28 UTC; the toolkit's directory written 00:38:40; done shortly after: about 8 to 9 minutes, most of it the download |
| size | `/usr/local/cuda-12.9`: 7.3 GB on disk |
| packages | about 215 new packages for `cuda-toolkit-12-9` (12.9.2): the compiler (`cuda-nvcc-12-9` 12.9.86), runtime and libraries (cuBLAS, cuFFT, cuRAND, cuSOLVER, cuSPARSE, NPP, nvJPEG, nvJitLink, cuFile), debugger and profilers |
| pulled in from Ubuntu | `build-essential` (gcc and g++ 13, which `nvcc` calls), and, for NVIDIA's graphical profilers (Nsight Systems 2025.1.3, Nsight Compute 2025.2.1, the Visual Profiler), a Java 21 runtime, GTK 3, Mesa and X libraries |
| links | `update-alternatives` points `/usr/local/cuda` and `/usr/local/cuda-12` at `/usr/local/cuda-12.9` |
| from NVIDIA's repository besides the toolkit | `cuda-toolkit-config-common` 13.4.92, a shared configuration package (no driver), which the low pin allows because only that repository has it |
| left for later | `needrestart` deferred restarts of `dbus`, `systemd-logind` and `unattended-upgrades`, and named the login session as running outdated binaries; a reboot or a new login clears it |

A headless compute node has no use for the graphical profilers or their Java
and desktop libraries. A lighter set is possible (`cuda-compiler-12-9`,
`cuda-libraries-dev-12-9`, `cuda-command-line-tools-12-9`), at the cost of
those tools; open item 4.

### The toolkit step that looked stopped

After `==> CUDA toolkit 12.9` the screen showed nothing for five minutes, and
Paul could find no activity. Read over SSH:

- the processes: `apt-get install -y -qq cuda-toolkit-12-9`, running 5 min
  13 s, with apt's `https` and `http` fetch methods alive;
- `/var/cache/apt/archives`: 3.1 GB, 239 packages;
- `eno1`: 3.16 GB received since boot;
- measured over ten seconds: **77 MB received (7.7 MB/s)**, packages
  239 → 244.

It was downloading, at a steady rate, with nothing on the screen: the script
passed `-qq` to `apt-get`, which hides apt's own fetch lines. The script now
installs the toolkit without `-qq` and says first that it is about 4 GB.
The first run kept the old behaviour and finished normally.

### After a reboot

Paul rebooted calber (`ssh -t user@calber sudo reboot`); the console watched
over the network and checked as `user`, without root:

| | |
| --- | --- |
| down, back | down 17:41:37 (Mac's clock); SSH answering 17:42:02, 25 s later; booted 00:41:47 UTC |
| kernel | 7.0.0-38-generic, as before |
| driver | `nvidia` in use for the GPU; no `nouveau` module loaded |
| `nvidia-smi` | `NVIDIA GeForce GTX 1080 Ti, 580.178.04, 11264 MiB, 35, P0` |
| services | 0 failed units (the deferred restarts cleared) |
| CUDA | `nvcc` on a new login's PATH (`/usr/local/cuda-12.9/bin/nvcc`); a program compiled for the card and run as `user`: `CUDA after reboot: NVIDIA GeForce GTX 1080 Ti, compute 6.1: correct` |

The node keeps all of it across a reboot, with nothing to do by hand.

### The login: Copal fleet, not Ubuntu's advertising

Ubuntu's login offered help links, news from motd.ubuntu.com, "Enable ESM
Apps to receive additional future security updates", and "New release
'26.04.1 LTS' available. Run 'do-release-upgrade'": the last one the very
upgrade this node must not take. `tools/fleet-login.sh --role cuda`, run on
the node as root:

- **turns off**, with `chmod -x` and a list kept for `--undo`:
  `10-help-text`, `50-motd-news` (and `ENABLED=0` for its timer),
  `91-contract-ua-esm-status`, `91-release-upgrade`; and Ubuntu Pro's
  `apt_news`, which puts the ESM advertising into apt's own output;
- **keeps** `50-landscape-sysinfo` (load, memory, temperature, address),
  updates waiting, reboot required, firmware, and the HWE end-of-life notice;
- **adds** `01-copal-fleet`, run at each login, reading
  `/etc/copal-fleet/node.conf`: the fleet's name, the node and its role, the
  GPU as it is (name, driver, temperature, load, memory), CUDA's version,
  what is held and why, hints, and a warning against `do-release-upgrade`;
- **holds the release** for a CUDA node: `Prompt=never` (section VI of the
  version report's recommendations).

Previewed on calber as `user` before installing:

```
  Copal fleet · orrery
  calber -- the fleet's CUDA node (Ubuntu 24.04.5 LTS, not Copal)
  GPU   NVIDIA GeForce GTX 1080 Ti · driver 580.178.04 · 33°C · 0% busy · 0 of 11264 MiB
  CUDA  12.9
  held: Ubuntu 24.04, driver 580, CUDA 12.9: held for a Pascal card

  Hints
    nvidia-smi                       the card, now
    nvcc --version                   the CUDA toolkit
    ~/code/burnprobe                 Burn on this card (orrery tools/burn-probe)
    sudo less /var/log/cuda-node.log how this node was set up

  Do not run do-release-upgrade: Ubuntu 26.04 has no CUDA 12 for this card.
```

**The first run stopped partway.** On calber it silenced the five scripts
and then exited without a word: the list it kept for `--undo` was built with
`cat old-list new-list | sort`, and on a first run there is no old list, so
`cat` failed, `pipefail` made the line fail, and `set -e` ended the script
before it wrote `node.conf`, held the release or installed the banner. The
list is gone (`--undo` turns every script in the silence list back on), and
the script was then **rehearsed whole on calber as `user`**: a copy with
every system path pointed into a scratch tree holding copies of calber's
files, run to the end (five silenced, news off, `Prompt=never`, `node.conf`,
`fleet-status`, the banner; exit 0), the banner run, and `--undo` run (all
five back, news on). Only then was it handed back to be run as root.

### The node at a glance: `fleet-status`

Paul asked for the login to show the node's address, the GPU, and what is
running, `htop`-like, especially scripts started remotely. That is
`tools/fleet-status.sh`, installed by `fleet-login.sh` as
`/usr/local/bin/fleet-status` and run by the login banner; also a command
(`watch -c -n 2 fleet-status` keeps it live). Ubuntu's own
`50-landscape-sysinfo` is now silenced too, since the panel says it.

| Panel | From |
| --- | --- |
| node, role, system, uptime, address and interface | `/etc/copal-fleet/node.conf`, `uptime`, `ip -4 addr` |
| CPU, memory, disk as coloured bars (green, yellow at 60%, red at 85%) | `/proc/stat` sampled over 0.2 s, `/proc/meminfo`, `df` |
| GPU: name, driver, temperature, power, load and memory bars, processes on it | `nvidia-smi` |
| remote sessions: who, from where, since when | `who` |
| **started over SSH**: every process under an SSH connection that is not sshd or a bare shell, with user, source address, age, CPU, command | the process tree; the address from the process's own `SSH_CONNECTION`, or the connection's `sshd` as `ss` names it (root) |
| busiest five by CPU | `ps` |

**Tested on calber** from the Mac: two SSH sessions each ran a CPU-bound
job (`yes`, and `bash -c "while :; do :; done"`), and a third ran the panel:

```
  CPU   ██████░░░░░░░░░░░░░░░░░░  24%   load 0.93 1.24 1.44 · 8 cores
  Started over SSH (scripts, builds, runs; not the shells themselves)
    PID     USER     FROM                 AGE    CPU  COMMAND
    15049   user     192.168.8.143      0m04s    99%  bash -c while :; do :; done
    15050   user     192.168.8.143      0m04s    99%  yes
    15046   user     192.168.8.143      0m04s     0%  timeout 25 bash -c while :; do :; done
    15047   user     192.168.8.143      0m04s     0%  bash -c timeout 25 yes > /dev/null
    15048   user     192.168.8.143      0m04s     0%  timeout 25 yes
```

(192.168.8.143 is the Mac.) The first version showed the source as `?`
(as a user, `ss` cannot name root's `sshd`), hid the busy `bash -c` (a
filter that dropped every `bash` without a `.sh`), and listed its own `sed`;
the environment's `SSH_CONNECTION`, a filter for bare shells only, and
skipping its own session fixed all three. The whole panel takes about 1.3 s.

### `topnotch`

Paul named the panel **topnotch** ("like the simple script to show me system
resources ... especially if they are more than 10% CPU"), and asked for
copal-tm beside it: copal-tm (vonglurt/copal-tm) is the Copal task manager,
a full-screen instrument panel and process browser; topnotch is the glance.

What changed from `fleet-status`:

- **CPU now, measured.** `ps`'s `%CPU` is a process's average over its
  life, so a job that went flat out a minute ago after an hour idle barely
  shows. topnotch reads every process's CPU ticks (`/proc/PID/stat`, utime
  plus stime, parsed after the `)` that ends a name that may hold spaces)
  twice, half a second apart, as htop does.
- **Busy now:** every process over 10% of a core in that half second,
  busiest first, with its user, memory, and the address it was started
  from if an SSH session started it.
- the SSH table shows CPU now, sorted by it.
- a hint for copal-tm: the command if it is installed, else how to install
  it (`cargo install copal-tm`; installing from crates.io on the node was left
  to Paul, who wrote it).

Tested on calber with two CPU-bound jobs started over SSH from the Mac:

```
  Busy now (over 10% of a core, measured over half a second)
    PID     USER        CPU    MEM  FROM             COMMAND
    46953   user       102%   0.0%  192.168.8.143    bash -c while :; do :; done
    46888   user       102%   0.0%  192.168.8.143    yes
```

(Half a second is a coarse window, so a single-threaded process can read a
little over 100%.) `fleet-login.sh` installs it as `/usr/local/bin/topnotch`,
removes the old `fleet-status`, and the banner runs it; rehearsed on calber
in a scratch tree, install and undo.

## VI. Discussion

**Ubuntu's installer did half the work.** "Third-party" put in the right
driver branch, built and signed by Ubuntu for the running kernel, which is
what a node needs to survive its own updates. What a script had to add was
the part Ubuntu does not ship (NVIDIA's toolkit) and a guard against the part
that could go wrong later (NVIDIA's repository offering a driver without
Pascal).

**Silence is not a stop, and a stop is not silence.** The same lesson as the
card build on saskatchewan the same day, from the other side: there, a
stop printed nothing; here, progress printed nothing. Both are answered the
same way: say what is happening, keep a log, and when unsure, measure the
machine (bytes received, packages fetched) before concluding.

**Toward a fleet verb.** The node is Ubuntu, so it is not in `copal fleet
state` and orrery cannot see it. The script is the first form of a verb
("make this a CUDA node"), run by hand over SSH. To be a verb of the
console, the fleet needs a way to hold a non-Copal node, its address,
account and role, and that is a decision for the fleet's design, not this
script.

### The second node, boburner

28 October 2026. boburner is 192.168.8.113:
- Ryzen 7 3700X, 16 threads, 31 GB;
- GTX 1070 Ti (Pascal, compute 6.1, 8 GB);
- an Ubuntu 24.04 desktop with a ZFS pool (`Hexamir`) and VirtualBox.

It came with Ubuntu's 535 driver and was months behind.

**1. The upgrade that would not finish.**
- `apt-get upgrade` held back the NVIDIA packages and the HWE kernel
  meta-packages. Four packages were left half-installed: 24.04's HWE
  kernel, which is now **7.0** (7.0.0-38), and its headers.
- Its DKMS step failed. zfs-dkms 2.2.2 does not configure on 7.0
  (`bdev_open_by_path() exists... configure: error`), and
  virtualbox-dkms 7.0.16 does not build.
- A machine whose ZFS pool and VMs depend on DKMS modules stays on the
  standard 6.8 line. So the HWE kernel and its meta-packages were
  purged, and `linux-generic` (6.8.0-147) put in their place.
- No dist-upgrade.

**2. The driver: 535 is now a name for 580.**
- Ubuntu's `nvidia-driver-535`, `nvidia-utils-535`,
  `nvidia-kernel-common-535` and `linux-modules-nvidia-535-generic`
  are now transitional packages. Each depends on its 580 counterpart.
- So `apt-get install linux-generic linux-modules-nvidia-535-generic
  nvidia-driver-535` (08:33, from `/var/log/apt/history.log`) installed
  driver 580.178.04, with Ubuntu's prebuilt, signed module for
  6.8.0-147 (`linux-modules-nvidia-580-generic`). After the reboot:

```
$ uname -r
6.8.0-147-generic
$ cat /proc/driver/nvidia/version
NVRM version: NVIDIA UNIX x86_64 Kernel Module  580.178.04
```

This is the desktop 580 branch, not calber's 580-server. Both are the
same driver version, from Ubuntu's archive. ZFS and VirtualBox rebuilt
for 6.8.0-147, and the pool is mounted.

**3. Chrome's repository.** It failed its signature check
(`NO_PUBKEY FD533C07C264648F`) and served a 404 for a version that is
gone. `apt-get purge google-chrome-stable` removed the package and its
repository list. Then `apt-get upgrade` finished, leaving 2 packages
phased: fwupd and linux-firmware.

**4. `cuda-node.sh`: right result, wrong report.** It ran twice
(15:38Z and 15:56Z). Both runs printed:

```
    installed branch: 535 (asked for 580-server); keeping what is there
    branch 535: a GTX 900 or 10 series card needs 580; newer cards are fine on it too
...
NVIDIA GeForce GTX 1070 Ti, 580.178.04, 8192 MiB, 36
device: NVIDIA GeForce GTX 1070 Ti, compute 6.1, 19 SMs, 7.9 GB
vector add of 1048576 floats: correct
==> Done: boburner is a CUDA node (driver 535, CUDA 12.9).
```

- CUDA 12.9 works: `nvcc` 12.9.86 compiled for sm_61 and ran on the card.
- But the script names the wrong branch.
  - It takes the first `nvidia-kernel-common-*` package dpkg lists, and
    the transitional `-535` sorts before `-580`.
  - Its own `nvidia-smi` line two steps later says 580.178.04.
  - It also installed `nvidia-utils-535`, a transitional name for what
    was there.
- The fix: read the branch from the loaded module
  (`/proc/driver/nvidia/version`), or from `nvidia-smi`, not from
  package names.
- An earlier note here (the same day) said the toolkit could not run
  on 535. That was not what happened: the card was on 580 throughout
  the script's runs. It is also not true in general, because CUDA 12's
  minor-version compatibility runs sm_61 code built by 12.9 on a 525+
  driver.
- nvcc 12.9 warns that offline compilation for targets before sm_75
  will be removed in a future release. On a Pascal card CUDA 12.9 is
  the last toolkit, as section III says.

**What is left behind, harmless.**
- **The four transitional 535 packages.** To remove them, mark the 580
  ones manual first (`apt-mark manual nvidia-driver-580 nvidia-utils-580
  linux-modules-nvidia-580-generic`), then purge the 535 names.
- **The module for the fallback kernel 6.8.0-52**
  (`linux-modules-nvidia-535-6.8.0-52-generic`).
- **Config-only (`rc`) entries of old 535 modules.**
- **A `dkms status` warning** that the zfs module built for 6.8.0-147
  differs from the installed one. The kernel ships its own zfs 2.2.2
  module beside zfs-dkms's.

## VII. Open items

1. Done: the toolkit and the CUDA check (V).
2. Running treadcore's GPU work on it (Burn, `AI-BRAINZ-PLAN.md` section 8)
   is a step of its own.
3. A non-Copal node in `copal fleet state`: how, if at all.
4. A headless toolkit set in `cuda-node.sh` (compiler, libraries, command-line
   tools), without the graphical profilers and their Java and desktop
   packages: smaller and faster, for nodes nobody sits at.
5. `cuda-node.sh` (V, boburner):
   - Read the driver branch from the loaded module, not the first
     `nvidia-kernel-common-*` package (Ubuntu's 535 names are now
     transitional to 580).
   - When no driver is present, choose the module package by the kernel
     line, not always HWE: a node with DKMS modules (ZFS, VirtualBox)
     stays on 6.8, since 24.04's HWE kernel is now 7.0.
