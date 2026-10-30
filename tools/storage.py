#!/usr/bin/env python3
"""storage.py QUEUE COMMAND ... -- the fleet's disks and shares: a patch bay.
    status                       the document orrery's Storage view draws (JSON)
    survey [NAME|all]            read each machine's disks, links and shares
                                 over ssh, as the user (no root): its block
                                 devices, NICs and their speed, ZFS pools,
                                 btrfs, exports, network mounts, the sharing
                                 tools it has
    stage-share NAME PATH        copy share-nfs.sh to NAME; print the command
                                 that exports PATH over NFS to the fleet's
                                 network, for its Sessions tile (root)
    stage-mount KIND SERVER PATH CLIENT [MOUNTPOINT]
                                 copy mount-share.sh to CLIENT; print the
                                 command that mounts SERVER:PATH there, now
                                 and at every boot (KIND nfs, sshfs or smb)
    stage-unmount CLIENT MOUNTPOINT
                                 the same, taking the mount away

Paul, 29 October: "a settings program in orrery which helps deploy and
configure and mount shares on various fleet members. Mounting eachothers
data. This is like a patch bay to ensure and monitor and add connnectivity
of disk." The survey reads; every change is a root script staged on the
machine and typed into its Sessions tile, where the person types the
password, as machines.py's recipes are. NOTHING HERE FORMATS A DISK: the
Ceph candidates are a list to choose from (docs/storage-lab-report.md), and
a disk becomes Ceph's only by a person's hand.

The registry is QUEUE/storage.json, beside machines.json (whose targets it
uses).
"""
import json
import os
import re
import shlex
import subprocess
import sys
import threading
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]
NET = "192.168.8.0/24"

# Read on the machine as the user: plain sh (busybox on Alpine), key=value
# lines and lsblk's JSON between markers.
SURVEY = r'''
k() { printf '%s=%s\n' "$1" "$2"; }
k hostname "$(hostname)"
k os "$(. /etc/os-release; echo $ID)"
k mem_gb "$(awk '/MemTotal/{printf "%.0f",$2/1048576}' /proc/meminfo)"
k cpus "$(nproc)"
for n in /sys/class/net/*; do b=${n##*/}; [ "$b" = lo ] && continue
  k "nic" "$b $(cat $n/speed 2>/dev/null || echo 0) $(cat $n/operstate 2>/dev/null) $(ip -4 -o addr show $b 2>/dev/null | awk '{print $4}' | head -1)"; done
k zpools "$(command -v zpool >/dev/null && zpool list -H -o name,size,alloc,free,health 2>/dev/null | tr '\t' ' ' | tr '\n' ';')"
k zfs_datasets "$(command -v zfs >/dev/null && zfs list -H -o name,used,avail,mountpoint 2>/dev/null | head -40 | tr '\t' ' ' | tr '\n' ';')"
k mounts "$(awk '$3 ~ /^(ext4|btrfs|xfs|zfs|vfat|exfat|ntfs3|ntfs|fuseblk|f2fs)$/ {print $1" "$2" "$3}' /proc/mounts | tr '\n' ';')"
k netmounts "$(awk '$3 ~ /^(nfs|nfs4|cifs|smb3|fuse.sshfs|ceph)$/ {print $1" "$2" "$3}' /proc/mounts | tr '\n' ';')"
k exports "$(grep -v '^#' /etc/exports 2>/dev/null | grep . | tr '\n' ';')"
k df "$(df -P -k 2>/dev/null | awk 'NR>1 && $1 ~ /^\// {print $6" "$2" "$4}' | tr '\n' ';')"
k tools "$(for t in exportfs mount.nfs smbd mount.cifs sshfs ceph rbd zpool btrfs mkfs.btrfs tgtd targetcli iscsiadm; do command -v $t >/dev/null && printf '%s ' $t; done)"
echo "lsblk<<"
lsblk -J -b -o NAME,SIZE,TYPE,ROTA,TRAN,FSTYPE,MOUNTPOINT,MODEL 2>/dev/null
echo ">>lsblk"
'''


def load(p, d):
    try:
        with open(p) as f:
            return json.load(f)
    except (OSError, ValueError):
        return d


def save(p, v):
    with open(p + ".tmp", "w") as f:
        json.dump(v, f, indent=1)
    os.replace(p + ".tmp", p)


def gib(n):
    return round(n / 2 ** 30, 1)


class Storage:
    def __init__(self, q):
        self.q = q
        self.path = os.path.join(q, "storage.json")
        self.reg = load(self.path, {})
        machines = load(os.path.join(q, "machines.json"), {})
        cfg = load(os.path.join(q, "config.json"), {})
        # Every machine with an address, the Mac and a CUDA lane's twin aside.
        self.targets = {}
        for h in cfg.get("hosts", []):
            if h.get("target") and h["target"] != "-" and not h.get("lane_of"):
                self.targets[h["name"]] = h["target"]
        for n, m in machines.items():
            if m.get("target") and m["target"] != "-" and not n.endswith("-cuda"):
                self.targets.setdefault(n, m["target"])

    def keep(self):
        save(self.path, self.reg)

    # ---- the survey --------------------------------------------------------------
    def survey_one(self, name):
        t = self.targets[name]
        m = self.reg.setdefault(name, {})
        m["target"] = t
        try:
            r = subprocess.run(SSH + [t, "sh -s"], input=SURVEY, capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired:
            m["error"] = "no answer in 60 s"
            return
        if r.returncode != 0 and not r.stdout:
            m["error"] = (r.stderr.strip().splitlines() or ["ssh failed"])[-1]
            return
        facts, nics = {}, []
        body = r.stdout
        blk = re.search(r"lsblk<<\n(.*)\n>>lsblk", body, re.S)
        for l in body.split("lsblk<<")[0].splitlines():
            if "=" in l:
                k, v = l.split("=", 1)
                if k == "nic":
                    p = v.split()
                    nics.append({"name": p[0], "mbps": int(p[1]) if p[1].lstrip("-").isdigit() and int(p[1]) > 0 else 0,
                                 "state": p[2] if len(p) > 2 else "", "addr": p[3] if len(p) > 3 else ""})
                else:
                    facts[k] = v.strip()
        disks = []
        try:
            for d in json.loads(blk.group(1)).get("blockdevices", []) if blk else []:
                # A snap's loop device, an optical drive, a RAM disk: not storage.
                if d.get("type") in ("loop", "rom") or d["name"].startswith(("zram", "ram", "loop", "sr")):
                    continue
                disks.append(self.disk(d, facts))
        except ValueError:
            pass
        m.update({"facts": facts, "nics": nics, "disks": disks, "surveyed": time.strftime("%Y-%m-%dT%H:%M:%S")})
        m.pop("error", None)

    @staticmethod
    def disk(d, facts):
        """A whole device and what it is for, read without root."""
        kids = d.get("children") or []
        mounts = [k.get("mountpoint") for k in [d] + kids if k.get("mountpoint")]
        fss = sorted({k.get("fstype") for k in [d] + kids if k.get("fstype")})
        size = int(d.get("size") or 0)
        rota = str(d.get("rota")) in ("1", "True", "true")
        tran = d.get("tran") or ""
        kind = "nvme" if tran == "nvme" or d["name"].startswith("nvme") else ("hdd" if rota else "ssd")
        out = {"name": d["name"], "size_gib": gib(size), "kind": kind, "tran": tran, "model": (d.get("model") or "").strip(),
               "fstype": ",".join(fss), "mounts": mounts, "parts": len(kids)}
        # What it is for: the system, a pool, in use, or free to choose.
        if d.get("type") != "disk" or size == 0 or tran == "usb":
            role, why = "skip", "not a fixed disk"
        elif any(mp in ("/", "/boot", "/boot/efi", "[SWAP]") for mp in mounts):
            role, why = "system", "holds the system or swap"
        elif "zfs_member" in fss:
            role, why = "zfs", "a member of a ZFS pool"
        elif "crypto_LUKS" in fss:
            role, why = "locked", "encrypted (LUKS): someone's data"
        elif mounts:
            role, why = "in use", "mounted at " + ", ".join(mounts)
        elif "btrfs" in fss:
            role, why = "idle pool", "btrfs, not mounted: may hold data; mount it, or wipe it by hand to give it to Ceph"
        elif fss:
            role, why = "candidate", "%s, not mounted: wiped by hand, a Ceph OSD" % ",".join(fss)
        else:
            role, why = "candidate", "no filesystem the user can read, not mounted (root's blkid says for sure)"
        out.update({"role": role, "why": why})
        return out

    def survey(self, names):
        threads = [threading.Thread(target=self.survey_one, args=(n,)) for n in names]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        self.keep()

    # ---- what the view draws -------------------------------------------------------
    def status(self):
        machines, exports, candidates = [], [], []
        for name in sorted(self.targets):
            m = self.reg.get(name, {})
            f = m.get("facts", {})
            nics = m.get("nics", [])
            fast = max([n["mbps"] for n in nics if n.get("state") == "up"] or [0])
            # The address on the fastest link that has one: whitewolf's 10 Gb
            # port is 192.168.8.195, its 1 Gb port the .124 the fleet knows
            # it by, and a share mounted by the fleet's address would never
            # use the 10 Gb (29 October).
            up = sorted((n for n in nics if n.get("state") == "up" and n.get("addr")), key=lambda n: -n["mbps"])
            addr = up[0]["addr"].split("/")[0] if up else self.targets[name].split("@")[-1]
            disks = m.get("disks", [])
            for e in [x for x in f.get("exports", "").split(";") if x.strip()]:
                exports.append({"server": name, "addr": addr, "path": e.split()[0], "line": e.strip(), "kind": "nfs"})
            mounts = []
            for nm in [x for x in f.get("netmounts", "").split(";") if x.strip()]:
                p = nm.split()
                mounts.append({"source": p[0], "at": p[1] if len(p) > 1 else "", "kind": p[2] if len(p) > 2 else ""})
            for d in disks:
                if d["role"] in ("candidate", "idle pool"):
                    candidates.append(dict(d, machine=name, mem_gb=int(f.get("mem_gb") or 0), mbps=fast, os=f.get("os", "")))
            machines.append({
                "name": name, "target": self.targets[name], "addr": addr, "os": f.get("os", ""),
                "mem_gb": int(f.get("mem_gb") or 0), "cpus": int(f.get("cpus") or 0), "mbps": fast, "nics": nics,
                "disks": disks, "zpools": [x for x in f.get("zpools", "").split(";") if x.strip()],
                "datasets": [x for x in f.get("zfs_datasets", "").split(";") if x.strip()],
                "tools": f.get("tools", "").split(), "mounts": mounts,
                "surveyed": m.get("surveyed", ""), "error": m.get("error", ""), "log": m.get("log", [])[-6:],
                # What Ceph would ask of it: a server needs the memory for
                # its OSDs (4 GiB each by default) and 10 Gb between them.
                "ceph": self.ceph_role(f, fast),
            })
        # Fastest first, then the largest: where an OSD does the most good.
        rank = {"nvme": 0, "ssd": 1, "hdd": 2}
        candidates.sort(key=lambda c: (rank.get(c["kind"], 3), -c["mbps"], -c["size_gib"]))
        return {"machines": machines, "exports": exports, "candidates": candidates, "net": NET}

    @staticmethod
    def ceph_role(f, mbps):
        mem = int(f.get("mem_gb") or 0)
        if f.get("os") == "ubuntu" and mem >= 24:
            return "server" + ("" if mbps >= 10000 else ": wants 10 Gb (has %s)" % (("%d Gb" % (mbps // 1000)) if mbps >= 1000 else "%d Mb" % mbps))
        if mem >= 24:
            return "server by hand (Alpine: no cephadm) or a client"
        return "client (%d GB: too little for an OSD)" % mem

    # ---- changes: root scripts, staged -----------------------------------------------
    def stage(self, name, files, command, what):
        t = self.targets.get(name)
        if not t:
            raise SystemExit("no machine %s" % name)
        r = subprocess.run(SSH + [t, "mkdir -p ~/orrery-recipes"], capture_output=True, text=True, timeout=30)
        if r.returncode != 0:
            raise SystemExit("cannot reach %s: %s" % (t, r.stderr.strip()[-200:]))
        r = subprocess.run(["scp", "-q", "-o", "BatchMode=yes"] + [os.path.join(TOOLS, f) for f in files] + ["%s:orrery-recipes/" % t],
                           capture_output=True, text=True, timeout=120)
        if r.returncode != 0:
            raise SystemExit("copying to %s failed: %s" % (t, r.stderr.strip()[-200:]))
        root = "doas" if self.reg.get(name, {}).get("facts", {}).get("os") == "alpine" else "sudo"
        cmd = "cd ~/orrery-recipes && %s sh %s" % (root, command)
        self.reg.setdefault(name, {}).setdefault("log", []).append({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "staged": what, "command": cmd})
        self.keep()
        return {"machine": name, "target": t, "command": cmd, "what": what}

    def stage_share(self, name, path):
        if not path.startswith("/") or path.rstrip("/") == "" or not re.fullmatch(r"[A-Za-z0-9_./-]+", path):
            raise SystemExit("a share is an absolute path of letters, digits, _ . / -: %s" % path)
        return self.stage(name, ["share-nfs.sh"], "share-nfs.sh %s %s" % (shlex.quote(path), NET), "share %s over NFS" % path)

    def stage_mount(self, kind, server, path, client, at=None, undo=False):
        if kind not in ("nfs", "sshfs", "smb"):
            raise SystemExit("a mount is nfs, sshfs or smb")
        if server not in self.targets:
            raise SystemExit("no machine %s" % server)
        at = at or "/mnt/fleet/%s%s" % (server, path.replace("/", "-").rstrip("-") if path != "/" else "")
        if not re.fullmatch(r"[A-Za-z0-9_./-]+", at) or not re.fullmatch(r"[A-Za-z0-9_./-]+", path):
            raise SystemExit("paths of letters, digits, _ . / - only")
        st = self.status()
        addr = next(m["addr"] for m in st["machines"] if m["name"] == server)
        src = "%s:%s" % (addr, path) if kind != "smb" else "//%s/%s" % (addr, path.strip("/").split("/")[-1])
        user = self.targets[server].split("@")[0]
        cmd = "mount-share.sh %s %s %s %s%s" % (kind, shlex.quote(src), shlex.quote(at), user, " --undo" if undo else "")
        return self.stage(client, ["mount-share.sh"], cmd, "%s %s:%s at %s" % ("unmount" if undo else "mount " + kind, server, path, at))


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    s, cmd, a = Storage(argv[1]), argv[2], argv[3:]
    if cmd == "status":
        print(json.dumps(s.status()))
    elif cmd == "survey":
        names = sorted(s.targets) if not a or a[0] == "all" else a
        for n in names:
            if n not in s.targets:
                sys.exit("no machine %s" % n)
        s.survey(names)
        st = s.status()
        bad = [m["name"] for m in st["machines"] if m["error"]]
        print("surveyed %d machine(s): %d disks, %d Ceph candidates, %d shares%s" % (
            len(names) - len(bad), sum(len(m["disks"]) for m in st["machines"]), len(st["candidates"]), len(st["exports"]),
            "; no answer from " + ", ".join(bad) if bad else ""))
    elif cmd == "stage-share" and len(a) == 2:
        print(json.dumps(s.stage_share(a[0], a[1])))
    elif cmd == "stage-mount" and len(a) in (4, 5):
        print(json.dumps(s.stage_mount(a[0], a[1], a[2], a[3], a[4] if len(a) > 4 else None)))
    elif cmd == "stage-unmount" and len(a) == 2:
        m = [x for x in s.status()["machines"] if x["name"] == a[0]]
        mt = next((x for x in (m[0]["mounts"] if m else []) if x["at"] == a[1]), None)
        if not mt:
            sys.exit("%s has no network mount at %s" % (a[0], a[1]))
        print(json.dumps(s.stage(a[0], ["mount-share.sh"], "mount-share.sh %s %s %s user --undo" % (
            {"nfs4": "nfs", "nfs": "nfs", "fuse.sshfs": "sshfs", "cifs": "smb", "smb3": "smb"}.get(mt["kind"], "nfs"),
            shlex.quote(mt["source"]), shlex.quote(a[1])), "unmount %s" % a[1])))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
