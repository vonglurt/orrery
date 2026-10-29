#!/usr/bin/env python3
"""machines.py QUEUE COMMAND ... -- the fleet's machines, from a fresh install
to a node: what each one is, what has been applied to it, and the recipes
that make it a node. orrery's Machines view reads and drives it.

    status                  every machine and its recipes, as JSON (no ssh)
    probe [NAME|all]        read each machine over ssh, as the user: its OS,
                            CPU, memory, GPU, driver, CUDA, Rust, checkout,
                            tmux, copal-tm, login banner, power-off rule;
                            from those, each recipe's state
    add NAME USER@ADDRESS   a machine not in the fleet yet (a fresh install
                            with OpenSSH and your key: ssh-copy-id first)
    details RECIPE NAME     what applying it would do, as JSON: the files and
                            their sha256, the command, whether it applies
    stage RECIPE NAME       copy the recipe's files to ~/orrery-recipes/ on the
                            machine; print the command to type (as JSON)
    join NAME [--yes]       add it to the queue's config.json and the fleet's
                            nodes list (orrery's Sessions)

THE RECIPES ARE ORRERY'S OWN SCRIPTS (tools/), and nothing else: each needs
root, and root is never run from here. `stage` copies; the command is typed
into the machine's tmux session by orrery's Sessions view, where the person
types the sudo password (Paul, 28 October: "Type it in a Sessions tile").
Every recipe checks before it acts, so running one again is safe; a recipe's
state ("applied") is read by `probe` from what it leaves behind, as the user,
with no root.

The registry is QUEUE/machines.json. Machines in the queue's config.json are
in it by being there.
"""
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]

# Each recipe: what it is, on which systems, what it needs, its files (from
# tools/), the command run on the machine as root, and what it does.
RECIPES = [
    {"id": "fleet-node", "title": "Make it a fleet node",
     "os": ["ubuntu 24.04"],
     "files": ["fleet-node.sh", "cuda-node.sh", "fleet-login.sh", "topnotch.sh", "fleet-poweroff-setup.sh"],
     "run": "bash fleet-node.sh",
     "does": ["the build tools (build-essential, pkg-config, git, curl, tmux)",
              "Rust for the user (rustup), in their home",
              "~/code/treadcore over HTTPS, and ~/fleet",
              "with an NVIDIA card: cuda-node.sh (the driver kept or installed, CUDA 12.9, the check)",
              "tmux and copal-tm",
              "the login banner (topnotch)",
              "the power-off rule (sudoers: poweroff, nothing else)"]},
    {"id": "cuda-node", "title": "CUDA: the driver, CUDA 12.9, tmux, copal-tm",
     "os": ["ubuntu 24.04"], "needs": "nvidia",
     "files": ["cuda-node.sh"], "run": "bash cuda-node.sh",
     "does": ["Ubuntu's 580 driver branch: kept if installed, else installed (prebuilt, signed modules)",
              "NVIDIA's CUDA repository at a low apt priority (it can never replace Ubuntu's driver)",
              "the CUDA 12.9 toolkit, and its PATH for every login",
              "the check: nvidia-smi and a CUDA program compiled for the card",
              "tmux, and copal-tm for the user"]},
    {"id": "fleet-login", "title": "The login banner (topnotch)",
     "os": ["ubuntu"],
     "files": ["fleet-login.sh", "topnotch.sh"], "run": "bash fleet-login.sh --role {role}",
     "does": ["/usr/local/bin/topnotch and the login message 01-copal-fleet: CPU, memory, disk, the GPU now, SSH work",
              "Ubuntu's advertising in the login message turned off (fleet-login.sh --undo puts it back)"]},
    {"id": "poweroff", "title": "Let the fleet power it off",
     "os": ["ubuntu", "alpine"],
     "files": ["fleet-poweroff-setup.sh"], "run": "sh fleet-poweroff-setup.sh",
     "does": ["one rule: the user may run poweroff as root without a password, and nothing else"]},
]

# Read on the machine as the user: plain sh (busybox on Alpine), key=value.
PROBE = r'''
k() { printf '%s=%s\n' "$1" "$2"; }
. /etc/os-release 2>/dev/null
k os_id "${ID:-}"; k os_version "${VERSION_ID:-}"; k pretty "${PRETTY_NAME:-}"
k kernel "$(uname -r)"; k arch "$(uname -m)"
k cpus "$(grep -c ^processor /proc/cpuinfo)"
k cpu_model "$(grep -m1 'model name' /proc/cpuinfo | sed 's/.*: //')"
k mem_gb "$(awk '/MemTotal/ {printf "%.0f", $2/1048576}' /proc/meminfo)"
k disk "$(df -h / | awk 'NR==2 {print $3 " of " $2}')"
k addrs "$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $2 " " $4}' | tr '\n' ';')"
g=""; for d in /sys/bus/pci/devices/*; do
  [ "$(cat $d/vendor 2>/dev/null)" = 0x10de ] || continue
  case "$(cat $d/class 2>/dev/null)" in 0x03*) g=$(basename $d);; esac
done
k nvidia_pci "$g"
k gpu "$(command -v lspci >/dev/null && lspci 2>/dev/null | grep -iE 'vga|3d' | grep -i nvidia | sed 's/.*: //' | head -n1)"
k nvidia_driver "$(sed -n 's/.*Kernel Module *\([0-9.]*\).*/\1/p' /proc/driver/nvidia/version 2>/dev/null | head -n1)"
k nvidia_smi "$(command -v nvidia-smi >/dev/null && nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -n1)"
k cuda "$(ls -d /usr/local/cuda-*/bin/nvcc 2>/dev/null | sed 's|/usr/local/cuda-\([^/]*\)/.*|\1|' | awk '{print length($0), $0}' | sort -n | tail -n1 | cut -d' ' -f2)"
k cargo "$( ( [ -x ~/.cargo/bin/cargo ] && ~/.cargo/bin/cargo --version ) || ( command -v cargo >/dev/null && cargo --version ) )"
k treadcore "$(git -C ~/code/treadcore log --oneline -1 2>/dev/null | cut -c1-7)"
k tmux "$(command -v tmux)"
k screen "$(command -v screen)"
k copal_tm "$( ( [ -x ~/.local/bin/copal-tm ] && echo ~/.local/bin/copal-tm ) || command -v copal-tm)"
k topnotch "$( [ -x /usr/local/bin/topnotch ] && echo yes)"
# The power-off command exactly as the rule allows it (fleet-poweroff-setup.sh):
# sudo lists it, for a NOPASSWD rule, to the user; doas's rule file is root's
# alone, but that it exists can be seen.
p=$(sudo -n -l 2>/dev/null | sed -n 's/.*NOPASSWD: *\(.*poweroff.*\)$/\1/p' | head -n1)
if [ -n "$p" ]; then k poweroff "sudo -n $p"
elif [ -e /etc/doas.d/fleet-poweroff.conf ]; then k poweroff "doas -n $(command -v poweroff || echo /sbin/poweroff)"
else k poweroff ""; fi
k uptime "$(uptime | sed 's/.*up *//; s/, *[0-9]* user.*//')"
'''


# This Mac, the coordinator (a host whose target is "-"): read here, with
# macOS's own tools, since it has no /proc and no ssh address.
LOCAL_PROBE = r'''
k() { printf '%s=%s\n' "$1" "$2"; }
k os_id macos; k os_version "$(sw_vers -productVersion)"; k pretty "macOS $(sw_vers -productVersion)"
k hostname "$(scutil --get LocalHostName 2>/dev/null || hostname -s)"
k kernel "$(uname -r)"; k arch "$(uname -m)"
k cpus "$(sysctl -n hw.ncpu)"; k cpu_model "$(sysctl -n machdep.cpu.brand_string)"
k mem_gb "$(( $(sysctl -n hw.memsize) / 1073741824 ))"
k disk "$(df -h /System/Volumes/Data 2>/dev/null | awk 'NR==2 {print $3 " of " $2}')"
a=""; for i in en0 en1 en2; do x=$(ipconfig getifaddr $i 2>/dev/null) && a="$a$i $x;"; done; k addrs "$a"
k gpu "$(system_profiler SPDisplaysDataType 2>/dev/null | sed -n 's/^ *Chipset Model: *//p' | head -n1)"
PATH="$PATH:$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin"
k cargo "$(command -v cargo >/dev/null && cargo --version)"
k treadcore "$(git -C ~/code/treadcore log --oneline -1 2>/dev/null | cut -c1-7)"
k tmux "$(command -v tmux)"; k copal_tm "$(command -v copal-tm)"
k orrery_app "$( [ -d ~/Applications/Orrery.app ] && echo yes)"
k copal_tm_app "$( [ -d ~/Applications/'Copal TM.app' ] && echo yes)"
k uptime "$(uptime | sed 's/.*up *//; s/, *[0-9]* user.*//')"
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


class Machines:
    def __init__(self, q):
        self.q = os.path.abspath(q)
        self.cfg = load(os.path.join(self.q, "config.json"), {})
        self.reg_path = os.path.join(self.q, "machines.json")
        self.reg = load(self.reg_path, {})
        # Machines in the queue are in the registry by being there.
        for h in self.cfg.get("hosts", []):
            m = self.reg.setdefault(h["name"], {})
            m["target"] = h.get("target") or "-"
        self.fleet = self.cfg.get("fleet", "grove")

    def keep(self):
        save(self.reg_path, self.reg)

    def joined(self, name):
        return any(h["name"] == name for h in self.cfg.get("hosts", []))

    # ---- facts ------------------------------------------------------------------
    def probe(self, name):
        m = self.reg[name]
        try:
            if m["target"] == "-":
                r = subprocess.run(["bash", "-c", LOCAL_PROBE], capture_output=True, text=True, timeout=60)
            else:
                r = subprocess.run(SSH + [m["target"], PROBE], capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired:
            m["probe_error"] = "no answer in 60 s"
            return
        if r.returncode != 0 and not r.stdout:
            m["probe_error"] = (r.stderr.strip().splitlines() or ["ssh failed"])[-1]
            return
        facts = {}
        for l in r.stdout.splitlines():
            if "=" in l:
                key, v = l.split("=", 1)
                facts[key] = v.strip()
        m["facts"] = facts
        m["probed"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        m.pop("probe_error", None)

    # ---- recipes ------------------------------------------------------------------
    @staticmethod
    def recipe(rid):
        for r in RECIPES:
            if r["id"] == rid:
                return r
        raise SystemExit("no recipe %s (there are: %s)" % (rid, ", ".join(r["id"] for r in RECIPES)))

    @staticmethod
    def system(f):
        return "%s %s" % (f.get("os_id", ""), f.get("os_version", ""))

    def applies(self, r, f):
        """(yes, why): whether a recipe is for this machine, from its facts."""
        if not f:
            return False, "not probed yet"
        sysname = self.system(f)
        if not any(sysname == o or sysname.startswith(o + " ") or f.get("os_id") == o for o in r["os"]):
            return False, "for %s; this is %s" % (" or ".join(r["os"]), f.get("pretty") or sysname)
        if r.get("needs") == "nvidia" and not f.get("nvidia_pci"):
            return False, "needs an NVIDIA card; none on the PCI bus"
        return True, ""

    @staticmethod
    def applied(rid, f):
        """Whether its effects are there, read without root."""
        nv = bool(f.get("nvidia_pci"))
        have = {
            "fleet-login": bool(f.get("topnotch")),
            "poweroff": bool(f.get("poweroff")),
            "cuda-node": bool(f.get("nvidia_smi") and f.get("cuda") and f.get("tmux") and f.get("copal_tm")),
        }
        have["fleet-node"] = bool(f.get("cargo") and f.get("treadcore") and f.get("tmux") and f.get("copal_tm")
                                  and have["fleet-login"] and have["poweroff"] and (not nv or have["cuda-node"]))
        return have.get(rid, False)

    def recipe_states(self, name):
        f = self.reg[name].get("facts") or {}
        out = []
        for r in RECIPES:
            ok, why = self.applies(r, f)
            state = "applied" if ok and self.applied(r["id"], f) else "not applied" if ok else "not for it"
            # Everything the confirmation shows, so it needs no second call.
            out.append({"id": r["id"], "title": r["title"], "state": state, "why": why, "root": True,
                        "command": self.command(r, name), "does": r["does"],
                        "files": [{"name": fn, "sha256": self.sha(fn)} for fn in r["files"]]})
        return out

    _sums = {}

    @classmethod
    def sha(cls, fn):
        if fn not in cls._sums:
            cls._sums[fn] = hashlib.sha256(open(os.path.join(TOOLS, fn), "rb").read()).hexdigest()
        return cls._sums[fn]

    def root_cmd(self, f):
        return "doas" if f.get("os_id") == "alpine" else "sudo"

    def command(self, r, name):
        f = self.reg[name].get("facts") or {}
        role = "cuda" if f.get("nvidia_pci") else "cpu"
        return "cd ~/orrery-recipes && %s %s" % (self.root_cmd(f), r["run"].format(role=role))

    def details(self, rid, name):
        r, m = self.recipe(rid), self.reg[name]
        f = m.get("facts") or {}
        ok, why = self.applies(r, f)
        files = []
        for fn in r["files"]:
            p = os.path.join(TOOLS, fn)
            files.append({"name": fn, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest(), "bytes": os.path.getsize(p)})
        return {"recipe": rid, "title": r["title"], "machine": name, "target": m["target"],
                "system": f.get("pretty", "not probed"), "gpu": f.get("gpu") or f.get("nvidia_smi") or "",
                "applies": ok, "why": why, "state": "applied" if self.applied(rid, f) else "not applied",
                "does": r["does"], "files": files, "to": "~/orrery-recipes/",
                "command": self.command(r, name), "root": self.root_cmd(f)}

    def stage(self, rid, name):
        d = self.details(rid, name)
        if not d["applies"]:
            raise SystemExit("%s is not for %s: %s" % (rid, name, d["why"]))
        t = self.reg[name]["target"]
        r = subprocess.run(SSH + [t, "mkdir -p ~/orrery-recipes"], capture_output=True, text=True, timeout=30)
        if r.returncode != 0:
            raise SystemExit("cannot reach %s: %s" % (t, r.stderr.strip()[-200:]))
        r = subprocess.run(["scp", "-q", "-o", "BatchMode=yes"] + [os.path.join(TOOLS, f["name"]) for f in d["files"]]
                           + ["%s:orrery-recipes/" % t], capture_output=True, text=True, timeout=120)
        if r.returncode != 0:
            raise SystemExit("copying to %s failed: %s" % (t, r.stderr.strip()[-200:]))
        # What arrived is what was shown: the sums read back on the machine.
        names = " ".join(shlex.quote(f["name"]) for f in d["files"])
        r = subprocess.run(SSH + [t, "cd ~/orrery-recipes && (sha256sum %s 2>/dev/null || shasum -a 256 %s)" % (names, names)],
                           capture_output=True, text=True, timeout=30)
        there = dict((l.split()[1], l.split()[0]) for l in r.stdout.splitlines() if len(l.split()) == 2)
        bad = [f["name"] for f in d["files"] if there.get(f["name"]) != f["sha256"]]
        if bad:
            raise SystemExit("on %s the sums of %s do not match what was sent" % (name, ", ".join(bad)))
        self.reg[name].setdefault("log", []).append({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "staged": rid, "command": d["command"]})
        self.keep()
        return {"machine": name, "target": t, "command": d["command"], "files": [f["name"] for f in d["files"]]}

    # ---- joining ------------------------------------------------------------------
    def join(self, name):
        m = self.reg[name]
        f = m.get("facts") or {}
        if self.joined(name):
            raise SystemExit("%s is in the queue already" % name)
        if not (f.get("cargo") and f.get("treadcore")):
            raise SystemExit("%s has no Rust or no treadcore checkout yet: apply fleet-node first, then probe" % name)
        caps = ["cpu", "gpu"] if f.get("nvidia_smi") and f.get("cuda") else ["cpu"]
        host = {"name": name, "target": m["target"], "caps": caps, "nice": 10}
        # The command exactly as the machine's rule allows it.
        if f.get("poweroff"):
            host["poweroff"] = f["poweroff"]
        cfg = load(os.path.join(self.q, "config.json"), {})
        cfg["hosts"].append(host)
        save(os.path.join(self.q, "config.json"), cfg)
        nodes = os.path.expanduser("~/.copal/fleets/%s/nodes" % self.fleet)
        addr = m["target"].split("@", 1)[-1]
        if os.path.exists(nodes) and not re.search(r"^%s\s" % re.escape(name), open(nodes).read(), re.M):
            with open(nodes, "a") as fh:
                fh.write("%-15s %s\n" % (name, addr))
        m.setdefault("log", []).append({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "joined": host})
        self.keep()
        return host

    # ---- crucible's lanes ------------------------------------------------------------
    # Paul, 28 October: "i want to see a button to add more gpu CUDA to the
    # mix, add crucible." A machine in the queue whose probe found an NVIDIA
    # card and CUDA gets a second lane, NAME-cuda, caps ["cuda"]: the same
    # machine, taking only crucible's segments, so its CPUs go on with
    # treadcore's censuses. It has no power-off of its own; the machine's own
    # lane powers it off.
    def cuda_ready(self):
        """(name, why not or ''): every joined machine, and whether it can
        take a crucible lane."""
        out = []
        for h in self.cfg.get("hosts", []):
            if "cuda" in h.get("caps", []):
                continue
            f = (self.reg.get(h["name"]) or {}).get("facts") or {}
            why = "" if f.get("nvidia_smi") and f.get("cuda") else ("no NVIDIA card" if not f.get("nvidia_pci") else "no CUDA yet: apply cuda-node")
            out.append((h["name"], why))
        return out

    def cuda_lanes(self, how, which="all"):
        cfg = load(os.path.join(self.q, "config.json"), {})
        hosts = cfg.get("hosts", [])
        have = {h["name"] for h in hosts}
        done = []
        if how == "add":
            for name, why in self.cuda_ready():
                if why or (which != "all" and name != which) or name + "-cuda" in have:
                    continue
                me = next(h for h in hosts if h["name"] == name)
                hosts.append({"name": name + "-cuda", "target": me["target"], "caps": ["cuda"], "nice": 10, "lane_of": name})
                done.append(name + "-cuda")
        elif how == "remove":
            keep = [h for h in hosts if not ("cuda" in h.get("caps", []) and (which == "all" or h.get("lane_of") == which))]
            done = [h["name"] for h in hosts if h not in keep]
            hosts[:] = keep
        else:
            raise SystemExit("cuda-lanes add|remove [NAME|all]")
        if done:
            cfg["hosts"] = hosts
            save(os.path.join(self.q, "config.json"), cfg)
            for n in done:
                m = self.reg.get(n[:-5] if n.endswith("-cuda") else n)
                if m is not None:
                    m.setdefault("log", []).append({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "cuda_lane": how})
            self.keep()
        return done

    def status(self):
        out = []
        for name in sorted(self.reg, key=lambda n: (self.joined(n), n)):
            m = self.reg[name]
            f = m.get("facts") or {}
            out.append({"name": name, "target": m.get("target", ""), "joined": self.joined(name),
                         "probed": m.get("probed", ""), "error": m.get("probe_error", ""), "facts": f,
                         "recipes": self.recipe_states(name), "log": m.get("log", [])[-5:],
                         "cuda_lane": any(h.get("lane_of") == name for h in self.cfg.get("hosts", []))})
        ready = [n for n, why in self.cuda_ready() if not why]
        lanes = [h["name"] for h in self.cfg.get("hosts", []) if "cuda" in h.get("caps", [])]
        return {"fleet": self.fleet, "machines": out, "cuda_ready": ready, "cuda_lanes": lanes}


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    ms, cmd = Machines(argv[1]), argv[2]
    if cmd == "status":
        print(json.dumps(ms.status()))
    elif cmd == "probe":
        names = list(ms.reg) if len(argv) < 4 or argv[3] == "all" else [argv[3]]
        for n in names:
            if n not in ms.reg:
                sys.exit("no machine %s" % n)
            ms.probe(n)
            m = ms.reg[n]
            print("%-13s %s" % (n, m.get("probe_error") or "%s; %s" % (m["facts"].get("pretty"), m["facts"].get("gpu") or "no NVIDIA card")))
        ms.keep()
    elif cmd == "add":
        if len(argv) < 5 or "@" not in argv[4] or not re.match(r"^[A-Za-z0-9][A-Za-z0-9-]*$", argv[3]):
            sys.exit("add NAME USER@ADDRESS (a name of letters, digits and hyphens)")
        ms.reg.setdefault(argv[3], {})["target"] = argv[4]
        ms.probe(argv[3])
        ms.keep()
        print(json.dumps(ms.status()["machines"][[m["name"] for m in ms.status()["machines"]].index(argv[3])]["recipes"]))
    elif cmd == "details":
        print(json.dumps(ms.details(argv[3], argv[4])))
    elif cmd == "stage":
        print(json.dumps(ms.stage(argv[3], argv[4])))
    elif cmd == "join":
        print(json.dumps(ms.join(argv[3])))
    elif cmd == "cuda-lanes":
        how = argv[3] if len(argv) > 3 else "list"
        if how == "list":
            for n, why in ms.cuda_ready():
                lane = any(h.get("lane_of") == n for h in ms.cfg.get("hosts", []))
                print("%-13s %s" % (n, "crucible lane" if lane else (why or "ready for a crucible lane")))
        else:
            done = ms.cuda_lanes(how, argv[4] if len(argv) > 4 else "all")
            print("%s: %s" % ("added" if how == "add" else "removed", ", ".join(done) or "nothing to do"))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
