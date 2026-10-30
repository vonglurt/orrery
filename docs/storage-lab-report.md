# Shared Storage for the Fleet: What the Disks Are, the One Way Through, and the NVMe for a Small Ceph

*Lab Report — IEEE Format · 29 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `tools/storage.py`, `tools/share-nfs.sh`, `tools/mount-share.sh`;
`src/storage.rs` (the Storage view); `docs/fleet-train.md`.

---

## Abstract

Paul asked for shared storage across the fleet:
- a small Ceph cluster that every machine contributes a little to;
- direct shares of the ZFS and btrfs pools already there;
- a way in for the Mac, the Asustor NAS and a virtual machine;
- all of it driven from orrery, "like a patch bay to ensure and monitor
  and add connectivity of disk".

The fleet was surveyed first, as the user, with no root and nothing
changed:
- **9 machines, 45 disks and 20 Ceph candidates**, among them twelve
  disks in two **btrfs pools that are not mounted** (calber's six SATA
  SSDs, whitewolf's six 1 TB drives).
- **One ZFS pool:** boburner's Hexamir, 7.25 TB, 3.3 TB used.
- **The 10 Gb backbone is two machines today**, boburner and whitewolf
  (Mellanox ConnectX). **jaguaribe has the same card, its link down.**
- **whitewolf's 10 Gb port has its own address** (192.168.8.195). The
  fleet reaches it at the 1 Gb port's .124.

The recommendation is one way through for each job:
- **NFS 4.2 with `nconnect`** between the Linux machines: the kernel's
  own server, no encryption on a trusted network, several connections a
  mount.
- **SMB** only for the Mac and the Asustor.
- **sshfs** as the way that needs nothing installed on the server.
- **Ceph (CephFS)** for the small replicated space every machine keeps.
  It runs on the Ubuntu machines (cephadm), with **one NVMe OSD on each
  of three nodes with 10 Gb**, and the Copal machines mount it as
  clients.

orrery gains a **Storage** view (Machines → Storage):
- a survey;
- each machine's links and disks, and what each disk is for;
- a Ceph candidates page, ranked fastest first;
- a **patch bay**, a row a share and a column a machine, whose **+**
  mounts a share;
- Share… and Unmount… buttons. Each is staged as a root script and typed
  into the machine's Sessions tile, for the person's password.

Nothing in it formats a disk.

## I. Objective

1. Find what the fleet's disks and links are, without root.
2. Choose the fastest, lowest-overhead way to share for this network.
3. Plan the small Ceph cluster, and the NVMe it needs.
4. Give orrery the views and buttons to share, mount and watch.
5. Say how the Asustor, the Mac, a VM and XCP-ng fit.

## II. Materials

| Machine | System | Memory | Links up | Disks (survey) |
| --- | --- | --- | --- | --- |
| boburner | Ubuntu 24.04 | 31 GB | **10 Gb** (ConnectX), 1 Gb | ZFS pool Hexamir: 6 × 4 TB HDD; 4 TB NVMe (the system) |
| whitewolf | Ubuntu 24.04 | 63 GB | **10 Gb** (ConnectX, .195), 1 Gb (.124) | 6 × 1 TB HDD, **btrfs, not mounted**; 256 GB NVMe (the system) |
| calber | Ubuntu 24.04 | 63 GB | 1 Gb | 6 × 480 GB SATA SSD, **btrfs, not mounted**; 2 TB NVMe (the system); 1 TB NVMe **LUKS-encrypted** |
| saskatchewan | Alpine (Copal) | 63 GB | 2.5 Gb | 4 TB NVMe, 1 TB NVMe, 2 TB SATA SSD (all mounted, nearly empty); 2 TB HDD; 2 TB NVMe (the system) |
| jaguaribe | Alpine (Copal) | 47 GB | 1 Gb; **10 Gb ConnectX, down** | 6 × 2 TB SAS HDD, 2 × 480 GB SSD, not mounted; 1 TB HDD mounted |
| columbia | Alpine (Copal) | 31 GB | 1 Gb | 512 GB NVMe (the system) |
| nasser | Alpine (Copal) | 8 GB | 1 Gb | 250 GB SSD (the system) |
| xinanjiang | Alpine (Copal) | 8 GB | 1 Gb | 250 GB SSD (the system), 128 GB SSD (swap) |
| the Mac | macOS | — | — | the coordinator; a client |

Read by `tools/storage.py survey` over SSH (`lsblk -J`,
`/sys/class/net/*/speed`, `zpool list`, `/proc/mounts`, `/etc/exports`)
in about 5 seconds for all eight Linux machines. The card names come from
each port's kernel driver (`mlx4_core` is Mellanox ConnectX, `r8169`
Realtek, `e1000e` Intel). Alpine's `lsblk` does not report filesystems
without root, so a Copal disk with no visible filesystem is marked "root's
blkid says for sure".

## III. What the survey found

**Two btrfs pools are idle.** On calber and on whitewolf, all six disks
report btrfs, and none is mounted. Each set is most likely one six-device
btrfs filesystem, which would hold about 2.8 TB of SSD and about 6 TB of
HDD before any redundancy. They may hold data, so they are marked **idle
pool** (amber), not candidate. The first step is to mount them and look
(`btrfs filesystem show`, as root), not to wipe them.

**The ZFS pool is healthy and half full.** Hexamir on boburner: 7.25 TB,
3.32 TB used, ONLINE, on the 10 Gb machine. It is the fleet's best
existing share source.

**jaguaribe has eight unclaimed disks.** Six 2 TB SAS drives and two
480 GB SSDs hold partitions but nothing mounted. They are the largest pool
of free capacity: 12 TB raw on HDD, 0.96 TB on SSD.

**saskatchewan's flash is mounted but empty.** It has 3.7 TB free of 4 TB
on one NVMe, 932 GB of 1 TB on another, and 1.9 TB of 2 TB on a SATA SSD.
They are mounted under Windows-style "New Volume" names, which suggests
filesystems from another system. They are "in use", so they are not
candidates until a person decides otherwise.

**The links decide more than the disks.** Speeds of one link:

| | MB/s | Its share of a 10 Gb link |
| --- | --- | --- |
| 1 Gb Ethernet | ~117 | 10% |
| a 7,200 rpm HDD | ~180 | 15% |
| 2.5 Gb Ethernet | ~290 | 25% |
| a SATA SSD | ~550 | 47% |
| **10 Gb Ethernet** | **~1,170** | 100% |
| an NVMe (PCIe 3) | ~3,000 | 256% |

On 1 Gb, any one disk outruns the network. Only boburner and whitewolf
can move more than one SATA SSD's worth of data. A Ceph write with three
copies sends the data twice more between servers, so on 1 Gb a replicated
write tops out near 40–55 MB/s.

## IV. The one way through, for each job

| Job | The way | Why | The alternatives, and why not |
| --- | --- | --- | --- |
| Linux machine to Linux machine, files | **NFS 4.2, `nconnect=8`, `noatime`, `hard`** | The kernel's own server and client, no encryption cost on a trusted LAN. `nconnect` spreads a mount over several TCP connections, which is what fills a fast link [3]. Measured elsewhere: NFS ahead of SMB on Linux, and far ahead on metadata-heavy work [2], [3] | **SMB:** more overhead on Linux, historically one thread a connection. **sshfs:** encrypted and one stream, the slowest (the encrypted winner only [2]). **iSCSI:** a block device for one host at a time, not a share |
| The small shared space every machine keeps | **CephFS** (Ceph, replicated ×3) | Survives a machine going away; every machine reads the same tree | **NFS from one machine:** one machine is a single point of failure |
| Mac (Finder) | **SMB 3** from boburner (Samba on Hexamir) | Native in Finder, Time Machine capable | NFS works too (`mount -t nfs -o resvport,vers=4`), but Finder handles it poorly. **SFTP** (FileZilla): works now, to any machine, with nothing installed |
| The Asustor mirror | **rsync over SSH, nightly, from a ZFS snapshot** | ADM speaks rsync and SSH. A snapshot makes the copy consistent; only changes move | **NFS mount plus cp:** no consistency, no change-only copy |
| A VM (UTM on the Mac, or XCP-ng) | **an NFS client** like any machine, or **an XCP-ng NFS storage repository** | NFS SRs are thin and file-based; XCP-ng supports them first-class | **iSCSI SR:** thick LVM on a zvol, more to manage, no gain on this network |
| Nothing installed on the server | **sshfs** | Needs only sshd and the client's key | — |

**Why not iSCSI on the 10 Gb backbone for XCP-ng?** It works: LIO
(`targetcli`) can export a ZFS zvol from boburner. But an XCP-ng iSCSI
repository is thick LVM, one host owns it at a time, and on this network
it gains nothing over NFS. iSCSI is worth it when a block device must look
local, such as a VM's boot disk on a host with no disk of its own.

## V. The small Ceph: where it runs, and what NVMe it wants

**Where the daemons run.** Ceph's packages exist on Alpine, but no
deployment tool works there. The Alpine wiki's own procedure is a manual
bootstrap with OpenRC services, marked work in progress [1]. Ceph's
standard tool, cephadm, wants systemd and containers. So:
- **Servers (MON, MGR, MDS, OSD): the Ubuntu machines.** boburner,
  whitewolf and calber have 31, 63 and 63 GB of memory. cephadm deploys
  and upgrades them.
- **Clients: the Copal machines.** They mount CephFS with the kernel
  client (`mount -t ceph`, Alpine's `ceph-common`). jaguaribe and
  saskatchewan could be OSD hosts by hand later, by the wiki's procedure.

**The sizing rules** [4], [5]:
- three nodes at least, an odd count of monitors;
- about 4 GB of RAM an OSD (`osd_memory_target`), and 1–2 GB for each
  monitor and manager;
- 4–6 CPU threads for each NVMe OSD;
- 10 Gb between the nodes at least.

**The NVMe plan.** One PCIe NVMe added to each of three 10 Gb nodes, a
whole device each, BlueStore:

| Node | Link | The NVMe added | Ceph's use of it |
| --- | --- | --- | --- |
| boburner | 10 Gb | 1 TB, PCIe ×4 | an OSD (4 GB RAM); a MON, a MGR |
| whitewolf | 10 Gb (.195) | 1 TB, PCIe ×4 | an OSD; a MON, an MDS |
| jaguaribe | 10 Gb once cabled | 1 TB, PCIe ×4 | an OSD (by hand on Alpine, or a client); a MON |
| calber | 1 Gb (a 10 Gb card wanted) | its six SATA SSDs, if the btrfs pool is retired | six OSDs; or a client |

| Measure | 3 × 1 TB NVMe, replicated ×3 |
| --- | --- |
| Raw | 3 TB |
| Usable (one copy of 3, the 85% full ratio kept free) | ~0.85 TB |
| Survives | one node lost, still writable (`min_size` 2) |
| A write, at best | ~1.1 GB/s from a 10 Gb client, less two replica hops between servers |
| RAM it takes | ~4 GB an OSD, ~2 GB for each MON and MGR |

**"Each keeps a small amount" maps to Ceph this way.** Every server gives
one device (or an LVM volume of one) as an OSD. The pool keeps three
copies spread across hosts (`failure domain: host`). Every machine, server
or client, mounts the same CephFS tree. A machine on 1 Gb can be a client
but should not hold an OSD. Its replica traffic, and its share of
recovery after a failure, would run at a tenth of the speed and hold the
rest back.

**What it should hold:** the fleet's small shared state. That means the
queue's results, census binaries, model slots, configuration and
datasets that every machine reads. It should not hold bulk assets: those
stay on Hexamir and the btrfs pools, shared over NFS and mirrored to the
Asustor.

## VI. What was built: the Storage view and its tools

**`tools/storage.py`**:
- **survey:** reads each machine as the user;
- **status:** the document the view draws, with each machine's role and
  the candidates ranked;
- **stage-share, stage-mount, stage-unmount:** copy a root script to the
  machine and print the command that goes into its Sessions tile.

Each mount uses the address on the server's fastest link. That puts
whitewolf at .195, so its 10 Gb port carries the data.

**`tools/share-nfs.sh PATH NET`** (root):
- installs the NFS server if it is missing (Alpine `nfs-utils`, Ubuntu
  `nfs-kernel-server`);
- writes one export line, `rw,sync,no_subtree_check,root_squash`, for
  the fleet's /24 alone;
- starts the service at boot;
- refuses `/`, and `--undo` takes the share away.

**`tools/mount-share.sh KIND SOURCE MOUNTPOINT USER`** (root):
- **nfs:** `nfs4 vers=4.2,nconnect=8,hard,noatime,_netdev`;
- **sshfs:** with the user's key, `allow_other`, `reconnect`;
- **smb:** SMB 3.1.1, with a credentials file the person writes first.

One `fstab` line each, mounted now and at every boot (OpenRC's netmount,
or systemd). The mount point must be under `/mnt`, `/media` or `/srv`,
and `--undo` takes it away.

**The Storage view** (`src/storage.rs`, Machines → Storage):

| Part | What it shows, what it does |
| --- | --- |
| Machines (left) | each machine's fastest link, memory, disk count, free disks, Ceph role; **Survey all** |
| The machine (right) | its links; its ZFS pools; each disk with its kind, size and role (green candidate, amber idle pool); its network mounts with **Unmount…**; **share a path** and **Share…** |
| Ceph (left, below) | every candidate disk in the fleet, fastest first, with its machine's link and memory and why it is a candidate |
| The patch bay (below) | a row a share, a column a machine: "source", "● mounted", or **+** to mount it there, by nfs, sshfs or smb |
| The bottom line | what the last step did: "surveyed 8 machine(s): 45 disks, 20 Ceph candidates, 0 shares", or "…copied; its command is typed in whitewolf's Sessions tile, for the password" |

**Every change is confirmed and staged, never run from the console.**
Share…, + and Unmount… open a confirmation that says what will happen and
on which machine. Stage it copies the script and types its command into
that machine's Sessions tile, where the person types the password. That is
the rule the Machines view already keeps.

### Tests

| What | How | Result |
| --- | --- | --- |
| The survey | all eight Linux machines over SSH | 5 s; 45 disks, 20 candidates; boburner's snap loop devices excluded (at first they made it 141 "disks") |
| `share-nfs.sh` | Alpine 3.22 container, the services stubbed | one export line, existing lines kept; a second run adds nothing; `/` refused; `--undo` removes exactly its line |
| `mount-share.sh` | the same | one `fstab` line each for nfs and sshfs; a second run adds nothing; smb stops without its credentials file; a mount point under `/etc` refused; `--undo` clean |
| Staging | for real: boburner's `/Hexamir`, mounted on whitewolf | the script copied; the commands `sudo sh share-nfs.sh /Hexamir 192.168.8.0/24` and `sudo sh mount-share.sh nfs 192.168.8.113:/Hexamir /mnt/fleet/boburner-Hexamir user`, waiting for the password |
| The view | `--frame-view storage` with the real survey | the layout fixed twice: the bay was first drawn at the top, and the list could not hold eight machines |
| orrery | `cargo test` | 354 pass (two new: the document read, and sizes and speeds as a person says them) |

The NFS server and the mounts were not run against a real kernel here,
because a container has no NFS server to run. The first real run is
boburner's `/Hexamir`, which is staged and waiting.

## VII. The order of work

1. **Share Hexamir** (staged): boburner's `/Hexamir`, over NFS, mounted
   on whitewolf, 10 Gb to 10 Gb. Measure it with `fio` and `nfsstat -m`,
   with and without `nconnect`.
2. **Cable jaguaribe's ConnectX** (eth2) to the switch's SFP+ (fiber or
   DAC) port. That makes three 10 Gb machines. Consider moving whitewolf's
   fleet address to .195, so the keeper's own traffic uses the fast port.
3. **Open the idle btrfs pools** on calber and whitewolf: mount, look,
   then share them or retire them.
4. **Add the three NVMe**, then deploy Ceph with cephadm on the Ubuntu
   nodes: three MONs, an OSD each, CephFS. The Copal machines mount it as
   clients. Ceph buttons go into the Storage view only after the first
   deployment has been run by hand.
5. **The Asustor mirror:** nightly `zfs snapshot`, then `rsync -aH
   --delete` over SSH from the snapshot to the Asustor's rsync module.
6. **Samba on boburner** for the Mac, and an NFS SR for XCP-ng if it
   joins.

## VIII. Discussion

**The network is the storage system here.** Every disk in the fleet
outruns its machine's link but boburner's and whitewolf's. A design built
for the disks would be built for a speed the wires cannot carry. So the
choices follow the links:
- NFS, because it adds the least to each byte;
- Ceph only between 10 Gb nodes;
- the 1 Gb machines as clients, which is what their links let them be.

**The most valuable finds were not disks.** The finds that matter most
for the backbone are three facts about links:
- jaguaribe's 10 Gb card, unplugged;
- whitewolf's 10 Gb port at an address the fleet never uses;
- saskatchewan at 2.5 Gb, where the plan assumed 10.

Each costs a cable or an address, not money.

**A list, not a button, for the disks.** Turning a disk into a Ceph OSD
erases it. The survey cannot see filesystems on Copal without root, and
two of the largest sets of disks are btrfs pools that may hold data. So
the console lists candidates and says why. It does not offer to format
them. The Ceph buttons come after a person has deployed the first cluster
by hand and the steps are known on this hardware.

**Still open:**
- Filesystems on Copal disks need root's `blkid`. A root survey would
  replace "unknown".
- The patch bay knows NFS exports. Samba shares and CephFS are not yet
  rows.
- No mount is watched for health between surveys. A stale NFS mount
  (`hard`) hangs the processes that touch it, so the survey should report
  mount health.

## References

[1] Alpine Linux wiki, "CEPH" (work in progress): the packages, manual
monitor and OSD bootstrap with OpenRC, `ceph-volume lvm create
--no-systemd`. https://wiki.alpinelinux.org/wiki/CEPH

[2] "NAS performance: NFS vs SMB vs SSHFS" (1 GbE, fio and rsync),
segmentfault.com. https://segmentfault.com/p/1210000047069281

[3] TrueNAS forums, "Poor NFSv4 performance" (nconnect: 97 to 455 MB/s)
and "NFS vs SMB network performance benchmarking", forums.truenas.com;
"Performance gains after switching from SMB to NFS for TrueNAS",
techenclave.com.

[4] Computing for Geeks, "Ceph homelab hardware requirements":
`osd_memory_target`, CPU threads an NVMe OSD, 10 Gb networking.
https://computingforgeeks.com/ceph-homelab-hardware-requirements/

[5] Proxmox forum, "FabU: can I use Ceph in a very small cluster?": three
nodes against four, memory a daemon. https://forum.proxmox.com/posts/733185/

[6] orrery `docs/copal-machines-lab-report.md`: the recipes and the
Sessions-tile rule the staging follows.
