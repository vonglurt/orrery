#!/usr/bin/env python3
"""orrery's plan for treadcore: what the fleet does for it, decided here
(treadcore LAB-REPORT-BRAIN.md 112 to 114).

    python3 -I tools/plans/treadcore.py QDIR plan N     up to N new jobs, as JSON
    python3 -I tools/plans/treadcore.py QDIR collect    what the finished jobs found
    python3 -I tools/plans/treadcore.py QDIR judged ID  (on_done of a judge job)
    python3 -I tools/plans/treadcore.py QDIR families   the families' verdicts so far
    python3 -I tools/plans/treadcore.py QDIR sync       every machine's ledger, one table

tools/fleet-queue.py is the engine (jobs, lanes, the keeper, stops); this
file is the plan it calls for treadcore. treadcore keeps only the work:
how one census plays (scripts/census.sh), the brain's tests, the ledger and
the fit (scripts/ledger.py, scripts/suggest.py), the trainer (tools/bc).
Everything that decides what runs where, and gathers what came back, is
here.

THE NEWEST COMMIT. plan.json's "commit" is a ref ("origin/main") or a hash.
A ref is read again at each top-up, after a fetch, so the fleet follows
what the coordinator pushed. Each host builds each commit once, in its own
folder (~/fleet/q-NAME/c/SHORT/: a worktree, the census binary, the
defaults); new jobs use the newest, and a job runs on the build it was
planned with. Before building, a host fast-forwards its own checkout
(~/code/treadcore) when it is clean and on main.

FOUR KINDS OF WORK share the one queue:

- weights: the scripted brain's named weights (weights.rs), a census trial
  each, picked by scripts/suggest.py from the ledger: the queue's own
  censuses and the fleet's history (plan.json "history"), only those read
  with the same weights.rs, the same seconds and map, 55 seeds or more. A
  trial past two errors above the defaults, no map past two below, is read
  twice on held-out seeds.
- families: a candidate's weights over every family of scenarios/families/,
  each held to its floor: the defaults, then candidates that passed both
  held-out reads (this queue's, then each converge run's, newest first);
  8 a job, side by side.
- net: the learned tank, a round trained on the GPU machine and judged by
  one census machine's census against the best. The official net changes
  only when Paul says.
- prepare: a host's build of a commit, before any of the above on it.

SYNC. `sync` reads every machine's own censuses with scripts/ledger.py ON
that machine (each row keeps its binary's hash), copies each machine's
converge runs' records (not their binaries), and joins every table with
the queues' ledgers into one (ledger.py --merge): build/ledger/fleet.tsv.
The keeper runs it on its own clock (config.json "sync"), so nothing is
gathered by hand.
"""
import base64
import csv
import glob
import json
import os
import re
import shutil
import statistics as st
import subprocess
import sys
import time

SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]


def load(p, default=None):
    try:
        with open(p) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def save(p, v):
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(v, f, indent=1)
    os.replace(tmp, p)


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def write_tsv(path, rows):
    if not rows:
        return False
    with open(path + ".tmp", "w") as f:
        w = csv.DictWriter(f, list(rows[0].keys()), delimiter="\t", lineterminator="\n")
        w.writeheader()
        w.writerows(rows)
    os.replace(path + ".tmp", path)
    return True


class Plan:
    def __init__(self, qdir):
        self.q = os.path.abspath(qdir)
        self.name = os.path.basename(self.q.rstrip("/"))
        self.cfg = load(os.path.join(self.q, "plan.json"), {})
        self.qcfg = load(os.path.join(self.q, "config.json"), {})
        self.hosts = self.qcfg.get("hosts", [])
        self.tc = os.path.expanduser(self.cfg.get("project", "~/code/treadcore"))
        self.pd = os.path.join(self.q, "plan")
        os.makedirs(self.pd, exist_ok=True)
        self.state = load(os.path.join(self.pd, "state.json"), {})
        for k, v in (("weights_n", 0), ("gen", 0), ("held_n", 0), ("round", 0), ("held_read", []),
                     ("families_n", 0), ("families_queued", []), ("commits", {})):
            self.state.setdefault(k, v)
        self.home = "fleet/q-%s" % self.name  # under each host's $HOME
        self._sha = None

    def keep(self):
        save(os.path.join(self.pd, "state.json"), self.state)

    def ids(self, *subs):
        out = set()
        for sub in subs:
            for f in glob.glob(os.path.join(self.q, sub, "**", "*.json"), recursive=True):
                out.add(re.sub(r"^\d+-", "", os.path.basename(f))[:-5])
        return out

    # ---- the commit -----------------------------------------------------------------
    def weights_rs(self, commit):
        r = run(["git", "-C", self.tc, "rev-parse", "%s:source/treadcore-brain/src/weights.rs" % commit])
        return r.stdout.strip() if r.returncode == 0 else ""

    def sha(self):
        """The commit new jobs build: plan.json's ref, read after a fetch."""
        if self._sha:
            return self._sha
        ref = self.cfg.get("commit", "origin/main")
        run(["git", "-C", self.tc, "fetch", "-q", "origin"], timeout=120)
        r = run(["git", "-C", self.tc, "rev-parse", ref + "^{commit}"])
        if r.returncode != 0:
            raise SystemExit("cannot resolve %s in %s: %s" % (ref, self.tc, r.stderr.strip()))
        self._sha = r.stdout.strip()
        c = self.state["commits"]
        if self._sha not in c:
            # Censuses read with the same weights.rs share one set of
            # defaults, so their rows can feed one fit.
            c[self._sha] = {"weights_rs": self.weights_rs(self._sha), "first_seen": time.strftime("%Y-%m-%dT%H:%M:%S")}
        return self._sha

    def short(self, sha=None):
        return (sha or self.sha())[:7]

    def cdir(self):
        return "~/%s/c/%s" % (self.home, self.short())

    # ---- preparing the hosts --------------------------------------------------------
    def prep_id(self, host):
        return "prep-%s-%s" % (host, self.short())

    def prepare_jobs(self):
        sha, short = self.sha(), self.short()
        jobs = []
        known = self.ids("queue", "running", "done", "failed", "stopped")
        for h in self.hosts:
            jid = self.prep_id(h["name"])
            if jid in known:
                continue
            cmd = (
                "set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:$PATH\n"
                "C=%s; mkdir -p $C\n"
                "cd ~/code/treadcore && git fetch -q origin\n"
                # The host's own checkout follows main when nothing in it would be lost.
                "if [ \"$(git rev-parse --abbrev-ref HEAD)\" = main ] && git diff --quiet && git diff --cached --quiet; then\n"
                "  git merge -q --ff-only origin/main && echo \"checkout: main at $(git rev-parse --short HEAD)\" || echo 'checkout: not a fast-forward, left as it is'\n"
                "else echo \"checkout: $(git rev-parse --abbrev-ref HEAD), with changes or not main: left as it is\"; fi\n"
                "git worktree remove --force $C/wt 2>/dev/null || true; git worktree prune\n"
                "git worktree add -q --detach $C/wt %s\n"
                "cd $C/wt && export CARGO_TARGET_DIR=$HOME/fleet/target && . scripts/common.sh && census_bin $C >/dev/null\n"
                "CENSUS_DUMP=$C/defaults.toml CENSUS_SEEDS=0 $C/census-bin --ignored brain_census >/dev/null 2>&1\n"
                "echo \"census binary %s: $( (sha256sum $C/census-bin 2>/dev/null || shasum -a 256 $C/census-bin) | cut -c1-16)\"\n"
            ) % (self.cdir(), sha, short)
            job = {"id": jid, "kind": "prepare", "title": "build %s on %s" % (short, h["name"]),
                   "needs": "host:" + h["name"], "cmd": cmd, "timeout_s": 3600}
            if "gpu" in h.get("caps", []):
                # The GPU machine also builds the trainer at the commit.
                job["cmd"] += (
                    "cd $C/wt/tools/bc && CARGO_TARGET_DIR=$HOME/code/treadcore/tools/bc/target cargo build -q --release\n"
                    "cp $HOME/code/treadcore/tools/bc/target/release/treadcore-bc $C/treadcore-bc\n"
                )
            if h["name"] == self.cfg["census_host"]:
                job["fetch"] = "%s/c/%s/defaults.toml" % (self.home, short)
            jobs.append(job)
        return jobs

    def prepared(self, host):
        return self.prep_id(host) in self.ids("done")

    def after_host(self):
        return "prep-{host}-%s" % self.short()

    # ---- the weights -----------------------------------------------------------------
    def census_cmd(self, trial_text, seeds, seed0, out_id):
        w = self.cfg["weights"]
        b = base64.b64encode(trial_text.encode()).decode()
        return (
            "set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:$PATH\n"
            "Q=~/%s; C=%s; mkdir -p $Q/trials; echo %s | base64 -d > $Q/trials/%s.weights\n"
            "cd $C/wt && export CARGO_TARGET_DIR=$HOME/fleet/target\n"
            "TRIAL=$Q/trials/%s.weights SEEDS=%d SEED0=%d SECS=%d MAP=%s RESPAWNS=3 JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) scripts/census.sh 2>&1 | tail -3\n"
            "d=$(ls -td build/census/*/ | head -1); s=$(basename $d)\n"
            "rm -rf $Q/out/%s; mkdir -p $Q/out/%s/$s; cp $d/*.txt $d/*.env $d/*.weights $d/g-*.log $Q/out/%s/$s/; rm -rf $d\n"
        ) % (self.home, self.cdir(), b, out_id, out_id, seeds, seed0, w["secs"], w["map"], out_id, out_id, out_id)

    def census_dirs(self):
        """Every fetched census of the queue: results/ID/fleet/q-NAME/out/ID/STAMP."""
        return sorted(glob.glob(os.path.join(self.q, "results", "*", self.home, "out", "*", "*", "")))

    def own_ledger(self):
        root = os.path.join(self.pd, "census")
        shutil.rmtree(root, ignore_errors=True)
        os.makedirs(root)
        for d in self.census_dirs():
            # The stamp, and the job: two hosts can finish in the same second.
            jid = d.rstrip("/").split(os.sep)[-2]
            os.symlink(d, os.path.join(root, "%s-%s" % (os.path.basename(d.rstrip("/")), jid)))
        out = os.path.join(self.pd, "ledger.tsv")
        with open(out, "w") as f:
            subprocess.run([sys.executable, "-I", os.path.join(self.tc, "scripts", "ledger.py"), root], stdout=f, check=True)
        return out

    def same_defaults(self, commit):
        """A row's commit read with today's weights.rs (and no '+': uncommitted source)."""
        if not commit or commit.endswith("+"):
            return False
        c = self.state["commits"]
        if commit not in c:
            c[commit] = {"weights_rs": self.weights_rs(commit)}
        return bool(c[commit]["weights_rs"]) and c[commit]["weights_rs"] == c[self.sha()]["weights_rs"]

    def ledger(self):
        """The fit's input: the queue's own rows, and the fleet's history rows
        that share today's defaults and the queue's census settings."""
        own = self.own_ledger()
        hist = self.cfg.get("history")
        if not hist:
            return own
        w = self.cfg["weights"]
        path = os.path.join(self.tc, hist) if not os.path.isabs(hist) else hist
        try:
            src = list(csv.DictReader(open(path), delimiter="\t"))
        except OSError:
            src = []
        q_stamps = {r["stamp"] for r in csv.DictReader(open(own), delimiter="\t")}
        rows = []
        for r in src:
            try:
                ok = (r["stamp"] not in q_stamps and int(r["seeds"] or 0) >= w["seeds"] and str(r["secs"]) == str(w["secs"])
                      and r["map"] == w["map"] and self.same_defaults(r["commit"]))
            except ValueError:
                ok = False
            if ok:
                rows.append(r)
        self.state["history_rows"] = len(rows)
        if not rows:
            return own
        h = os.path.join(self.pd, "history.tsv")
        write_tsv(h, rows)
        out = os.path.join(self.pd, "ledger-all.tsv")
        with open(out, "w") as f:
            subprocess.run([sys.executable, "-I", os.path.join(self.tc, "scripts", "ledger.py"), "--merge", h, own], stdout=f, check=True)
        return out

    def defaults(self):
        p = glob.glob(os.path.join(self.q, "results", self.prep_id(self.cfg["census_host"]), self.home, "c", self.short(), "defaults.toml"))
        return p[0] if p else None

    @staticmethod
    def passes(d):
        ok, below = False, False
        try:
            lines = open(os.path.join(d, "pooled.txt")).read().splitlines()
        except OSError:
            return None
        for l in lines:
            m = re.match(r"^(\S+(?: map)?)\s+([-+][\d.]+) \+/- ([\d.]+)", l)
            if not m:
                continue
            dd, se = float(m.group(2)), float(m.group(3))
            if m.group(1) == "every map":
                ok = se > 0 and dd > 2 * se
            elif se > 0 and dd < -2 * se:
                below = True
        return ok and not below

    def cpu_ready(self):
        return [h for h in self.hosts if "cpu" in h.get("caps", ["cpu"]) and self.prepared(h["name"])]

    def weight_jobs(self, n):
        w = self.cfg.get("weights")
        defaults = self.defaults() if w else None
        if not w or not defaults or not self.cpu_ready() or n <= 0:
            return []
        jobs = []
        # Held-out reads first, for trials that passed and have not been read.
        for d in self.census_dirs():
            jid = d.rstrip("/").split(os.sep)[-2]
            if not jid.startswith("w") or jid in self.state["held_read"] or not self.passes(d):
                continue
            trial = open(os.path.join(d, "trial.weights")).read()
            for k in range(2):
                hid = "h%04d" % self.state["held_n"]
                seed0 = w["held_seed0"] + 200000 * self.state["held_n"]
                self.state["held_n"] += 1
                jobs.append({"id": hid, "kind": "weights", "title": "held-out read %d of %s (%d seeds)" % (k + 1, jid, w["held_seeds"]),
                             "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(trial, w["held_seeds"], seed0, hid),
                             "fetch": "%s/out/%s" % (self.home, hid), "timeout_s": 6 * 3600})
            self.state["held_read"].append(jid)
        # Then new trials from the fit.
        want = n - len(jobs)
        if want > 0:
            g = self.state["gen"]
            out = os.path.join(self.pd, "gen%d" % g)
            shutil.rmtree(out, ignore_errors=True)
            r = run([sys.executable, "-I", os.path.join(self.tc, "scripts", "suggest.py"), self.ledger(), defaults, out,
                     "--n", str(want), "--seed", str(g)])
            if r.returncode != 0:
                raise SystemExit("suggest.py: " + r.stderr[-400:])
            self.state["gen"] += 1
            for f in sorted(glob.glob(os.path.join(out, "*.weights")))[:want]:
                text = open(f).read()
                jid = "w%04d" % self.state["weights_n"]
                seed0 = w["seed0"] + 10000 * self.state["weights_n"]
                self.state["weights_n"] += 1
                what = os.path.basename(f)[:-8]
                moved = "; ".join(l.split("#")[0].strip() for l in text.splitlines() if "=" in l.split("#")[0])[:80]
                jobs.append({"id": jid, "kind": "weights", "title": "%s: %s" % (what, moved or "the defaults"),
                             "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(text, w["seeds"], seed0, jid),
                             "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 6 * 3600})
        return jobs

    # ---- the families ------------------------------------------------------------------
    @staticmethod
    def weights_set(text):
        """A .weights file as SCENARIO_SET: name=value ... (comments dropped)."""
        out = []
        for l in text.splitlines():
            l = l.split("#")[0].strip()
            if "=" in l:
                k, v = (x.strip() for x in l.split("=", 1))
                out.append("%s=%s" % (k, v))
        return " ".join(out)

    def held_pairs(self):
        """Each trial's held-out reads: {trial: [census dir, ...]}, from the jobs' titles."""
        title = {}
        for f in glob.glob(os.path.join(self.q, "done", "**", "*.json"), recursive=True):
            j = load(f, {})
            m = re.match(r"held-out read \d of (w\d+)", j.get("title", ""))
            if m:
                title[j["id"]] = m.group(1)
        out = {}
        for d in self.census_dirs():
            hid = d.rstrip("/").split(os.sep)[-2]
            if hid in title:
                out.setdefault(title[hid], []).append(d)
        return out

    def family_candidates(self):
        """[(name, SCENARIO_SET)]: the defaults, this queue's candidates, then
        each converge run's CANDIDATE lines, newest generation first."""
        f = self.cfg.get("families") or {}
        out = [("defaults@%s" % self.short(), "")]
        if f.get("queue", True):
            for jid, ds in sorted(self.held_pairs().items()):
                if len(ds) == 2 and all(self.passes(d) for d in ds):
                    trial = glob.glob(os.path.join(self.q, "results", jid, self.home, "out", jid, "*", "trial.weights"))
                    if trial:
                        out.append(("%s/%s" % (self.name, jid), self.weights_set(open(trial[0]).read())))
        for run_dir in f.get("converge", []):
            run_dir = os.path.join(self.tc, run_dir) if not os.path.isabs(run_dir) else run_dir
            rows = []
            try:
                lines = open(os.path.join(run_dir, "candidates.txt")).readlines()
            except OSError:
                lines = []
            for l in lines:
                m = re.match(r"^(gen(\d+)/\S+) .*CANDIDATE", l)
                if m and os.path.exists(os.path.join(run_dir, m.group(1) + ".weights")):
                    rows.append((int(m.group(2)), m.group(1)))
            for g, name in sorted(rows, key=lambda r: (-r[0], r[1])):
                text = open(os.path.join(run_dir, name + ".weights")).read()
                out.append(("%s/%s" % (os.path.basename(run_dir.rstrip("/")), name), self.weights_set(text)))
        return out

    def family_jobs(self, n):
        f = self.cfg.get("families")
        if not f or not self.cpu_ready() or n <= 0:
            return []
        todo = [c for c in self.family_candidates() if c[0] not in self.state["families_queued"]]
        batch = int(f.get("batch", 8))
        jobs = []
        while todo and len(jobs) < n:
            part, todo = todo[:batch], todo[batch:]
            fid = "f%04d" % self.state["families_n"]
            self.state["families_n"] += 1
            b = base64.b64encode(("\n".join("%s\t%s" % c for c in part) + "\n").encode()).decode()
            # The test fails when a family is under its floor: that is a
            # verdict, kept in the output, not a failed job (the breaker).
            cmd = (
                "set -e; Q=~/%s; C=%s; O=$Q/out/%s; rm -rf $O; mkdir -p $O; echo %s | base64 -d > $O/candidates.tsv\n"
                "N=$(nproc 2>/dev/null || sysctl -n hw.ncpu)\n"
                "i=0; while IFS=$'\\t' read -r name set; do\n"
                "  i=$((i+1)); ( printf '%%s\\n%%s\\n' \"$name\" \"$set\" > $O/$i.set\n"
                "    t0=$(date +%%s); SCENARIO_SET=\"$set\" $C/census-bin --ignored every_family_holds_its_floor --nocapture > $O/$i.log 2>&1 && r=holds || r=UNDER\n"
                "    echo \"$r $(( $(date +%%s) - t0 ))\" > $O/$i.verdict ) &\n"
                "  while [ $(jobs -rp | wc -l) -ge $N ]; do sleep 2; done\n"
                "done < $O/candidates.tsv; wait\n"
                "for v in $O/*.verdict; do k=$(basename $v .verdict); echo \"$(cat $v) $(head -1 $O/$k.set)\"; done\n"
            ) % (self.home, self.cdir(), fid, b)
            jobs.append({"id": fid, "kind": "families", "title": "families: %s" % ", ".join(c[0] for c in part)[:110],
                         "needs": "cpu", "after_host": self.after_host(), "cmd": cmd,
                         "fetch": "%s/out/%s" % (self.home, fid), "timeout_s": 12 * 3600,
                         "on_done": "python3 -I %s %s families" % (os.path.abspath(__file__), self.q)})
            self.state["families_queued"] += [c[0] for c in part]
        return jobs

    def families(self):
        """Every families verdict fetched so far, into plan/families.json."""
        out = {}
        for d in sorted(glob.glob(os.path.join(self.q, "results", "f*", self.home, "out", "f*", ""))):
            for sf in glob.glob(os.path.join(d, "*.set")):
                k = sf[:-4]
                name, _, sset = open(sf).read().partition("\n")
                try:
                    verdict, secs = open(k + ".verdict").read().split()
                except (OSError, ValueError):
                    continue
                fams = []
                for l in open(k + ".log", errors="replace"):
                    m = re.match(r"^family (\S+) \[.*\]: held (\d+) of (\d+), ([\d.]+)% \+/- ([\d.]+)%, floor (\S+), (.+) \(([\d.]+) s\)", l)
                    if m:
                        fams.append({"family": m.group(1)[:-5], "held": int(m.group(2)), "runs": int(m.group(3)),
                                     "pct": float(m.group(4)), "se": float(m.group(5)), "floor": m.group(6), "verdict": m.group(7)})
                out[name] = {"set": sset.strip(), "verdict": verdict, "seconds": int(secs), "families": fams,
                             "job": d.rstrip("/").split(os.sep)[-1]}
        save(os.path.join(self.pd, "families.json"), out)
        held = [k for k, v in out.items() if v["verdict"] == "holds"]
        print("families: %d candidates read, %d hold every floor" % (len(out), len(held)))
        return out

    # ---- the net ------------------------------------------------------------------------
    def best_net(self):
        start = (self.cfg.get("net") or {}).get("start", "bc2.bin")
        p = os.path.join(self.pd, "best.bin")
        if not os.path.exists(p):
            shutil.copy(os.path.join(self.tc, "nets", start), p)
            save(os.path.join(self.pd, "best.json"), {"name": start, "round": 0})
        return p

    def net_jobs(self):
        n = self.cfg.get("net")
        if not n or not n.get("rounds"):
            return []
        gpu = [h for h in self.hosts if "gpu" in h.get("caps", [])]
        if not gpu or not self.prepared(gpu[0]["name"]) or not self.prepared(self.cfg["census_host"]):
            return []
        r = self.state["round"]
        # One round at a time: the next only when this one is judged (or failed).
        if r > 0 and "judge-%03d" % r not in self.ids("done", "failed", "stopped"):
            return []
        rounds = n["rounds"]
        if r >= len(rounds):
            return []
        spec = rounds[r]
        r += 1
        self.state["round"] = r
        store_q = "~/" + self.home
        tid, jid = "train-%03d" % r, "judge-%03d" % r
        best = self.best_net()
        data = (
            "D=%s/rounds/%03d; rm -rf $D; mkdir -p $D/data\n"
            "for f in ~/fleet/%s/shards/*.bin; do s=$(basename $f .bin); [ $s -ge %d ] && [ $s -lt %d ] && ln -s $f $D/data/; done; true\n"
            "ln -s ~/bc/rec60.bin $D/data/9000000.bin\n"
            "for s in 9001140 9001164 9001188 9001212 9001236; do ln -s ~/fleet/rec1200/shards/$s.bin $D/data/; done\n"
        ) % (store_q, r, spec["run"], spec["lo"], spec["lo"] + spec["n"])
        train = {
            "id": tid, "kind": "net", "title": "round %d: train the best on %s seeds %d to %d" % (r, spec["run"], spec["lo"], spec["lo"] + spec["n"] - 1),
            "needs": "gpu", "after": [self.prep_id(gpu[0]["name"])],
            "send": [{"src": best, "dst": "%s/rounds/%03d.start.bin" % (store_q, r)}],
            "cmd": data + "cd $D && HELD=9000054-9000059,9001140-9001259 START=%s/rounds/%03d.start.bin BC_DEVICE=cuda %s/treadcore-bc $D/data $D/cand.bin %s > $D/train.log 2>&1\n"
                   "grep -v 'DEINIT\\|^thread\\|^note\\|^$' $D/train.log | tail -14\n" % (store_q, r, self.cdir(), n["train_args"]),
            "fetch": "%s/rounds/%03d" % (self.home, r), "timeout_s": 4 * 3600,
        }
        c = n["census"]
        census_one = (
            "cd $C/wt && export CARGO_TARGET_DIR=$HOME/fleet/target\n"
            "SEEDS=%d SEED0=%d SECS=%d MAP=%s RESPAWNS=3 JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) CENSUS_NET=$Q/judge/%03d/%%s.bin scripts/census.sh 2>&1 | tail -2\n"
            "d=$(ls -td build/census/*/ | head -1); mkdir -p $Q/judge/%03d/%%s; cp $d/*.txt $d/*.env $d/g-*.log $Q/judge/%03d/%%s/; rm -rf $d\n"
        ) % (c["seeds"], c["seed0"], c["secs"], c["map"], r, r, r)
        judge = {
            "id": jid, "kind": "net", "title": "round %d: judge the candidate against the best, a census each" % r,
            "needs": "host:" + self.cfg["census_host"], "after": [tid, self.prep_id(self.cfg["census_host"])],
            "send": [{"src": os.path.join(self.q, "results", tid, self.home, "rounds", "%03d" % r, "cand.bin"), "dst": "~/%s/judge/%03d/cand.bin" % (self.home, r)},
                     {"src": best, "dst": "~/%s/judge/%03d/best.bin" % (self.home, r)}],
            "cmd": "set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:$PATH; Q=~/%s; C=%s\n" % (self.home, self.cdir())
                   + (census_one % ("cand", "cand", "cand")) + (census_one % ("best", "best", "best")),
            "fetch": "%s/judge/%03d" % (self.home, r),
            "on_done": "python3 -I %s %s judged %s" % (os.path.abspath(__file__), self.q, jid),
            "timeout_s": 6 * 3600,
        }
        return [train, judge]

    def judged(self, jid):
        r = int(jid.split("-")[1])
        base = os.path.join(self.q, "results", jid, self.home, "judge", "%03d" % r)

        def seeds(who):
            out = {}
            for f in glob.glob(os.path.join(base, who, "g-*.log")):
                s0 = int(re.search(r"g-(\d+)", f).group(1))
                for l in open(f):
                    l = re.sub(r"\x1b\[[0-9;]*m", "", l)
                    if l.startswith("brain_census: difference"):
                        for i, v in enumerate(l.split("a seed:")[1].split()):
                            out[s0 + i] = float(v)
            return out

        a, b = seeds("cand"), seeds("best")
        k = sorted(set(a) & set(b))
        if len(k) < 2:
            print("judge %s: too few paired seeds (%d)" % (jid, len(k)))
            return
        d = [a[s] - b[s] for s in k]
        m, se = st.mean(d), st.stdev(d) / len(d) ** 0.5
        verdict = {"round": r, "seeds": len(k), "cand": st.mean(a[s] for s in k), "best": st.mean(b[s] for s in k),
                   "diff": m, "se": se, "better": m > 2 * se}
        tlog = os.path.join(self.q, "results", "train-%03d" % r, self.home, "rounds", "%03d" % r, "train.log")
        agree = {}
        try:
            text = open(tlog).read()
            part = text.split("seeds 9000054-9000059")[1] if "seeds 9000054-9000059" in text else ""
            for h in ("drive", "gun", "mortar", "drone", "lead"):
                mm = re.search(r"^\s+%s\s+([\d.]+)%%" % h, part, re.M)
                if mm:
                    agree[h] = float(mm.group(1))
        except OSError:
            pass
        verdict["held_out"] = agree
        save(os.path.join(self.pd, "round-%03d.json" % r), verdict)
        if verdict["better"]:
            cand = os.path.join(self.q, "results", "train-%03d" % r, self.home, "rounds", "%03d" % r, "cand.bin")
            shutil.copy(cand, os.path.join(self.pd, "best.bin"))
            save(os.path.join(self.pd, "best.json"), {"name": "round %d" % r, "round": r, "census_vs_previous": [m, se], "held_out": agree})
        print("judge %s: candidate less best %+.2f +/- %.2f over %d seeds: %s" % (jid, m, se, len(k), "the new best" if verdict["better"] else "the best stays"))

    # ---- sync: every machine's record, gathered ------------------------------------------
    def sync(self, only=None):
        """Each machine's ledger, read on it; each machine's converge records
        (no binaries); every table joined into build/ledger/fleet.tsv.
        ONLY: that machine alone (before it powers off), then the join."""
        out = os.path.join(self.tc, "build", "ledger")
        os.makedirs(out, exist_ok=True)
        aliases = self.qcfg.get("aliases", {})
        tables, lines = [], []
        for h in self.hosts:
            name, target = h["name"], h["target"]
            p = os.path.join(out, "%s.tsv" % name)
            if only and name != only:
                if os.path.exists(p):
                    tables.append(p)
                continue
            cmd = "cd ~/code/treadcore 2>/dev/null && [ -d build/census ] && python3 -I scripts/ledger.py build/census || true"
            try:
                r = run(["bash", "-c", cmd] if target == "-" else SSH + [target, "bash -c %s" % json.dumps(cmd)], timeout=900)
                ok = r.returncode == 0
            except subprocess.TimeoutExpired:
                ok = False
            if ok:
                rows = list(csv.DictReader(r.stdout.splitlines(), delimiter="\t"))
                # One name a machine: run.env says what hostname(1) said,
                # which on a Mac changes with the network.
                for row in rows:
                    row["host"] = aliases.get(row["host"], row["host"])
                write_tsv(p, rows)
                lines.append("%s %d" % (name, len(rows)))
                # Its converge runs: the records the families and a person read.
                dst = os.path.join(self.tc, "build", "fleet-sync", name)
                if target == "-":
                    src = os.path.join(self.tc, "build", "converge")
                    if os.path.isdir(src):
                        os.makedirs(dst, exist_ok=True)
                        run(["rsync", "-a", "--exclude", "census-bin", src, dst + "/"], timeout=1800)
                elif run(SSH + [target, "test -d ~/code/treadcore/build/converge"], timeout=60).returncode == 0:
                    os.makedirs(dst, exist_ok=True)
                    run(["rsync", "-a", "--exclude", "census-bin", "-e", " ".join(SSH),
                         "%s:code/treadcore/build/converge" % target, dst + "/"], timeout=1800)
            else:
                lines.append("%s unreachable (its last table %s)" % (name, "kept" if os.path.exists(p) else "none"))
            if os.path.exists(p):
                tables.append(p)
        for q in sorted(glob.glob(os.path.join(self.tc, "build", "queue", "*", "plan", "ledger.tsv"))):
            name = q.split(os.sep)[-3]
            rows = list(csv.DictReader(open(q), delimiter="\t"))
            for row in rows:
                row["host"] = aliases.get(row["host"], row["host"])
            p = os.path.join(out, "queue-%s.tsv" % name)
            if write_tsv(p, rows):
                tables.append(p)
        fleet = os.path.join(out, "fleet.tsv")
        with open(fleet + ".tmp", "w") as f:
            subprocess.run([sys.executable, "-I", os.path.join(self.tc, "scripts", "ledger.py"), "--merge"] + tables, stdout=f, check=True)
        os.replace(fleet + ".tmp", fleet)
        by = {}
        for r in csv.DictReader(open(fleet), delimiter="\t"):
            by[r["host"]] = by.get(r["host"], 0) + 1
        summary = {"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "rows": sum(by.values()), "by_host": by, "hosts": lines}
        save(os.path.join(out, "sync.json"), summary)
        print("sync: fleet.tsv %d rows (%s); read: %s" % (summary["rows"], ", ".join("%s %d" % kv for kv in sorted(by.items())), "; ".join(lines)))

    # ---- the keeper's calls ---------------------------------------------------------------
    def plan(self, n):
        jobs = self.prepare_jobs()
        jobs += self.net_jobs()
        # The families first, a share of the queue: a candidate waits on them.
        jobs += self.family_jobs(max(0, (n - len(jobs) + 1) // 2))
        jobs += self.weight_jobs(max(0, n - len(jobs)))
        self.keep()
        print(json.dumps(jobs))

    def collect(self):
        rows = []
        for d in self.census_dirs():
            jid = d.rstrip("/").split(os.sep)[-2]
            p = self.passes(d)
            trial = ""
            try:
                trial = "; ".join(l.split("#")[0].strip() for l in open(os.path.join(d, "trial.weights")) if "=" in l.split("#")[0])
            except OSError:
                pass
            every = ""
            try:
                every = next(l for l in open(os.path.join(d, "pooled.txt")) if l.startswith("every map")).strip()
            except (OSError, StopIteration):
                pass
            rows.append({"job": jid, "trial": trial, "every_map": every, "passes": p})
        rounds = [load(f) for f in sorted(glob.glob(os.path.join(self.pd, "round-*.json")))]
        summary = {"weights": rows, "rounds": rounds, "best": load(os.path.join(self.pd, "best.json")), "families": self.families()}
        save(os.path.join(self.pd, "summary.json"), summary)
        passed = [r["job"] for r in rows if r["passes"]]
        print("collected: %d weight censuses (%d past two errors); %d net rounds judged" % (len(rows), len(passed), len(rounds)))


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    p = Plan(sys.argv[1])
    cmd = sys.argv[2]
    if cmd == "plan":
        p.plan(int(sys.argv[3]) if len(sys.argv) > 3 else 4)
    elif cmd == "collect":
        p.collect()
    elif cmd == "families":
        p.families()
    elif cmd == "judged":
        p.judged(sys.argv[3])
    elif cmd == "sync":
        p.sync(sys.argv[3] if len(sys.argv) > 3 else None)
    else:
        sys.exit(__doc__)
