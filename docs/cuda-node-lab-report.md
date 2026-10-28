# A CUDA Node for the Fleet: a GTX 1080 Ti on Ubuntu 24.04, Set Up over SSH

*Lab Report — IEEE Format · 27–28 October 2026 · interim: the toolkit install in progress*

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
and ends with a CUDA program compiled for the card and run. Its first run
looked stopped for five minutes; it was downloading at 7.7 MB/s with its
progress hidden, and the script now shows it.

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
| toolkit | **in progress** (below) |
| check | TOOLKIT_RESULT |

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
The run in progress keeps the old behaviour.

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

1. The toolkit's install and the CUDA check (TOOLKIT_RESULT).
2. Running treadcore's GPU work on it (Burn, `AI-BRAINZ-PLAN.md` section 8)
   is a step of its own.
3. A non-Copal node in `copal fleet state`: how, if at all.
