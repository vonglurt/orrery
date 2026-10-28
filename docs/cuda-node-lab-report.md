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

## VII. Open items

1. Done: the toolkit and the CUDA check (V).
2. Running treadcore's GPU work on it (Burn, `AI-BRAINZ-PLAN.md` section 8)
   is a step of its own.
3. A non-Copal node in `copal fleet state`: how, if at all.
4. A headless toolkit set in `cuda-node.sh` (compiler, libraries, command-line
   tools), without the graphical profilers and their Java and desktop
   packages: smaller and faster, for nodes nobody sits at.
