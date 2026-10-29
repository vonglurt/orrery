#!/usr/bin/env python3
"""orrery's plan for treadcore: what the fleet does for it, decided here
(treadcore LAB-REPORT-BRAIN.md 112 to 114).

    python3 -I tools/plans/treadcore.py QDIR plan N     up to N new jobs, as JSON
    python3 -I tools/plans/treadcore.py QDIR collect    what the finished jobs found
    python3 -I tools/plans/treadcore.py QDIR judged ID  (on_done of a judge job)
    python3 -I tools/plans/treadcore.py QDIR families   the families' verdicts so far
    python3 -I tools/plans/treadcore.py QDIR sync       every machine's ledger, one table
    python3 -I tools/plans/treadcore.py QDIR now        do it now: sync, then promote or re-read

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

PUSH PERMISSION. A template's plan.json names the remote and branch a
promotion lands on ("push": {"remote": "origin", "branch": "main"}), and the
coordinator must be able to push there: Approve is a push. It is checked by
a dry-run push at each sync and before every promotion and approval
(plan/push.json; `push-check`); without it no promotion starts.

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
import math
import random
import shlex
import glob
import hashlib
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


def env_file(p):
    out = {}
    try:
        for l in open(p):
            if "=" in l:
                k, v = l.rstrip("\n").split("=", 1)
                out[k] = v.strip("'\"")
    except OSError:
        pass
    return out


def parse_weights(text):
    """name = number lines of a weights file (caps and comments left out)."""
    out = {}
    for l in text.splitlines():
        l = l.split("#")[0].strip()
        m = re.match(r"^([a-z_][a-z0-9_]*)\s*=\s*([-+0-9.eE]+)$", l)
        if m:
            out[m.group(1)] = float(m.group(2))
    return out


def changed_vars(lines):
    """set-defaults.py's "name old -> new" lines as ticker vars; the
    Tactics::default() echo lines (and any old value that is an expression,
    DIRECT_RANGE - 1.0) are left out rather than read."""
    out = {}
    for l in lines:
        m = re.match(r"^([a-z_][a-z0-9_]*) ([-+0-9.eE]+) -> ([-+0-9.eE]+)$", l.strip())
        if m and not l.startswith(" "):
            out[m.group(1)] = {"value": float(m.group(3)), "offset": round(float(m.group(3)) - float(m.group(2)), 4)}
    return out


def short_moved(text, room=60):
    """A trial's weights for a job's title: whole names, "+N more" past the room."""
    parts = [l.split("#")[0].strip().replace(" = ", " ") for l in text.splitlines() if "=" in l.split("#")[0]]
    out = []
    for i, p in enumerate(parts):
        if len(", ".join(out + [p])) > room:
            return ", ".join(out) + ", +%d more" % (len(parts) - i)
        out.append(p)
    return ", ".join(out)


def github_web(url):
    """A remote's web page on GitHub, from any of the ways git spells it
    (git@github.com:o/r.git, ssh://git@ssh.github.com:443/o/r.git,
    https://github.com/o/r), or "" for a remote that is not GitHub."""
    m = re.search(r"github\.com[:/](?:\d+/)?([^/]+)/([^/]+?)(?:\.git)?/?$", url or "")
    return "https://github.com/%s/%s" % m.groups() if m else ""


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
        # treadcore's machines; a GPU machine's crucible lane (caps
        # ["cuda"], named NAME-cuda) is the same machine again, and builds
        # nothing of treadcore's.
        every = self.qcfg.get("hosts", [])
        self.hosts = [h for h in every if "cpu" in h.get("caps", ["cpu"])]
        self.cuda = [h for h in every if "cuda" in h.get("caps", [])]
        self.tc = os.path.expanduser(self.cfg.get("project", "~/code/treadcore"))
        self.pd = os.path.join(self.q, "plan")
        os.makedirs(self.pd, exist_ok=True)
        self.state = load(os.path.join(self.pd, "state.json"), {})
        for k, v in (("weights_n", 0), ("gen", 0), ("held_n", 0), ("round", 0), ("held_read", []),
                     ("families_n", 0), ("families_queued", []), ("commits", {})):
            self.state.setdefault(k, v)
        self.home = "fleet/q-%s" % self.name  # under each host's $HOME
        self._sha = None
        self._rsaves = {}
        self._newruns = []

    def flag(self, kind, text, key=None, progress=False):
        """A flag for a person (fleet-queue's flags/, the same form): raised once a key.
        progress=True: news, shown and never waiting for anyone (a candidate)."""
        d = os.path.join(self.q, "flags")
        os.makedirs(d, exist_ok=True)
        key = key or "%s:%s" % (kind, text)
        for f in glob.glob(os.path.join(d, "*.json")):
            j = load(f, {}) or {}
            if j.get("key") == key:
                # One raised before candidates were news: it becomes news.
                if progress and not j.get("progress"):
                    j["progress"] = True
                    save(f, j)
                return
        fid = "%d-%s" % (int(time.time() * 1000), kind)
        save(os.path.join(d, fid + ".json"), {"id": fid, "t": time.strftime("%Y-%m-%dT%H:%M:%S"), "kind": kind, "text": text, "key": key,
                                              "ack": None, "progress": bool(progress)})

    def keep(self):
        for rid, r in self._newruns:
            save(os.path.join(self.q, "runs", rid + ".json"), r)
        self._newruns = []
        save(os.path.join(self.pd, "state.json"), self.state)
        for rid, st in self._rsaves.items():
            save(os.path.join(self.rdir(rid), "state.json"), st)
        self._rsaves = {}

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

    def defaults_at(self, commit):
        """The weights! table's defaults at a commit: {name: number}, read
        from git, cached by weights.rs's blob."""
        blob = self.weights_rs(commit)
        if not blob:
            return {}
        cache = self.state.setdefault("tables", {})
        if blob not in cache:
            text = run(["git", "-C", self.tc, "cat-file", "-p", blob]).stdout
            table = {}
            for m in re.finditer(r'^\s*[A-Z][A-Za-z0-9]*\s+"([a-z_0-9]+)"\s+(-?[0-9][0-9.eE+-]*)\s*,', text, re.M):
                table[m.group(1)] = float(m.group(2))
            cache[blob] = table
        return cache[blob]

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
        # Self-modified promotions not yet on main: the fleet builds the
        # local line, main with them on top.
        line = self.local_line(self._sha) if ref == "origin/main" else None
        if line:
            self._sha = line
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

    def commits_log(self, n=40):
        """The line the fleet builds, newest first, each commit with the
        weights! defaults it changed (old, new), when the fleet first built
        it, and its page on GitHub (none for the local line's own commits,
        which are not there): plan/commits.json, for the console's Settings.
        A commit's changes never change, so each is read once (state)."""
        head = self.sha()
        remote, branch = self.push_target()
        web = github_web(run(["git", "-C", self.tc, "remote", "get-url", remote]).stdout.strip())
        local = set(run(["git", "-C", self.tc, "rev-list", "%s/%s..%s" % (remote, branch, head)]).stdout.split())
        r = run(["git", "-C", self.tc, "log", "--first-parent", "-n", str(n), "--format=%H%x09%P%x09%aI%x09%an%x09%s", head])
        seen = self.state.setdefault("commit_values", {})
        out = []
        for line in r.stdout.splitlines():
            parts = line.split("\t", 4)
            if len(parts) < 5:
                continue
            sha, parents, when, who, subject = parts
            if sha not in seen:
                parent = parents.split()[0] if parents else ""
                a, b = (self.defaults_at(parent) if parent else {}), self.defaults_at(sha)
                seen[sha] = [[k, a.get(k), b.get(k)] for k in sorted(set(a) | set(b)) if parent and a.get(k) != b.get(k)]
            out.append({"sha": sha, "short": sha[:7], "when": when[:19], "who": who, "subject": subject[:400],
                        "values": seen[sha], "built": (self.state["commits"].get(sha) or {}).get("first_seen"),
                        "local": sha in local, "url": "%s/commit/%s" % (web, sha) if web and sha not in local else ""})
        save(os.path.join(self.pd, "commits.json"), {"head": head, "branch": "%s/%s" % (remote, branch), "web": web,
                                                      "checked": time.strftime("%Y-%m-%dT%H:%M:%S"), "commits": out})

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
                # The local line's commits, when the commit is not on main yet.
                "[ -f $C/accepted.bundle ] && git fetch -q $C/accepted.bundle %s\n"
                "git worktree add -q --detach $C/wt %s\n"
                "cd $C/wt && export CARGO_TARGET_DIR=$HOME/fleet/target && . scripts/common.sh && census_bin $C >/dev/null\n"
                "CENSUS_DUMP=$C/defaults.toml CENSUS_SEEDS=0 $C/census-bin --ignored brain_census >/dev/null 2>&1\n"
                "echo \"census binary %s: $( (sha256sum $C/census-bin 2>/dev/null || shasum -a 256 $C/census-bin) | cut -c1-16)\"\n"
            ) % (self.cdir(), self.LINE, sha, short)
            job = {"id": jid, "kind": "prepare", "title": "build %s on %s" % (short, h["name"]),
                   "needs": "host:" + h["name"], "cmd": cmd, "timeout_s": 3600}
            if self.state.get("local_line"):
                job["send"] = [{"src": os.path.join(self.pd, "accepted.bundle"), "dst": "%s/accepted.bundle" % self.cdir()}]
                job["title"] += " (the local line: %d promotion(s) not on main)" % len(self.state["local_line"]["promotions"])
            if "gpu" in h.get("caps", []):
                # The GPU machine also builds the trainer at the commit.
                job["cmd"] += (
                    "cd $C/wt/tools/bc && CARGO_TARGET_DIR=$HOME/code/treadcore/tools/bc/target cargo build -q --release\n"
                    "cp $HOME/code/treadcore/tools/bc/target/release/treadcore-bc $C/treadcore-bc\n"
                )
            # Every machine brings back the defaults it built: after a
            # promotion, the proof that its values reached that machine.
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
            "TRIAL=$Q/trials/%s.weights SEEDS=%d SEED0=%d SECS=%d MAP=%s RESPAWNS=3 JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) CENSUS_BIN=$C/census-bin scripts/census.sh 2>&1 | tail -3\n"
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
            # Today's defaults only, as the history rows are: a census read
            # against the defaults before a promotion says nothing of a trial
            # against today's. suggest.py puts every row on today's log
            # scale, so the 182 rows read before w0034 was promoted (each
            # near +2, against the old defaults) went in as gains of the
            # weights that moved, and the fit's "best" chased them: read 309
            # times since, at -0.3 (29 October).
            if not self.same_defaults(self.census_commit(d)):
                continue
            # The stamp, and the job: two hosts can finish in the same second.
            jid = d.rstrip("/").split(os.sep)[-2]
            os.symlink(d, os.path.join(root, "%s-%s" % (os.path.basename(d.rstrip("/")), jid)))
        out = os.path.join(self.pd, "ledger.tsv")
        with open(out, "w") as f:
            subprocess.run([sys.executable, "-I", os.path.join(self.tc, "scripts", "ledger.py"), root], stdout=f, check=True)
        return out

    def same_defaults(self, commit):
        """A row's commit read with today's defaults: every weight both
        commits have at the same default (a weight added since, at a default
        that plays as before, keeps the history; a promotion, which moves a
        default, starts anew). No '+' (uncommitted source)."""
        if not commit or commit.endswith("+"):
            return False
        memo = self.state.setdefault("same", {})
        key = "%s:%s" % (commit, self.short())
        if key not in memo:
            a, b = self.defaults_at(commit), self.defaults_at(self.sha())
            common = set(a) & set(b)
            memo[key] = bool(common) and all(a[k] == b[k] for k in common)
        return memo[key]

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
        seq = self.sequential()
        # Held-out reads first, for trials the sequential read passed.
        for jid, v in sorted(seq.items()):
            if v["status"] != "passed" or jid in self.state["held_read"]:
                continue
            trial = v["text"]
            for k in range(2):
                hid = "h%04d" % self.state["held_n"]
                seed0 = w["held_seed0"] + 200000 * self.state["held_n"]
                self.state["held_n"] += 1
                jobs.append({"id": hid, "kind": "weights", "title": "held-out read %d of %s (%d seeds)" % (k + 1, jid, w["held_seeds"]),
                             "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(trial, w["held_seeds"], seed0, hid),
                             "fetch": "%s/out/%s" % (self.home, hid), "timeout_s": 6 * 3600})
            self.state["held_read"].append(jid)
        # Then the reads on, the most promising first: a test begun is
        # finished before most new ones begin (Fishtest's way). A third of
        # the room is kept for new trials, so a backlog of reads on (the
        # 1,100 trials read before this rule, say) does not stop the search.
        room = n - len(jobs)
        jobs += self.read_on_jobs(seq, room - room // 3, w)
        # Then new trials from the fit.
        want = n - len(jobs)
        if want > 0:
            s = self.search(defaults)
            s["sequential"] = self.seq_counts(seq)
            g = self.state["gen"]
            out = os.path.join(self.pd, "gen%d" % g)
            shutil.rmtree(out, ignore_errors=True)
            # Four times what is wanted: the trials already read are dropped
            # below, and the rest must still fill the generation.
            r = run([sys.executable, "-I", os.path.join(self.tc, "scripts", "suggest.py"), self.ledger(), defaults, out,
                     "--n", str(max(12, 4 * want)), "--seed", str(g), "--bound", str(s["bound"])])
            if r.returncode != 0:
                raise SystemExit("suggest.py: " + r.stderr[-400:])
            self.state["gen"] += 1
            # A stalled search gives half the generation to wide trials.
            wide = self.wide_trials(want // 2 or 1, out, g, defaults) if s["stalled"] else []
            fit = []
            try:
                fit = [os.path.join(out, l.split("\t")[0] + ".weights") for l in open(os.path.join(out, "plan.tsv")).read().splitlines()[1:]]
            except OSError:
                pass
            picked, now = [], set()
            for f in wide + fit:
                if len(picked) >= want:
                    break
                text = open(f).read()
                key = self.trial_key(text)
                # suggest.py's reads again are the sequential read's to make
                # (read_on_jobs, pooled with the first): not new trials.
                name = os.path.basename(f)
                if name.startswith("again-") or key in now:
                    continue
                now.add(key)
                # A trial once on today's defaults. suggest.py sees only what
                # has been read, so each generation, five minutes apart, asked
                # again for what was still queued: since w0300, 721 trials and
                # 310 different ones, one of them read 77 times (29 October).
                # The fit's best, which moves by a hair a generation: again
                # once there are 20 new reads to have moved it.
                if s["tried"].get(key) or (name.startswith("best") and s["reads"] < s.get("best_at", -99) + 20):
                    s["repeats"] = s.get("repeats", 0) + 1
                    continue
                s["tried"][key] = s["tried"].get(key, 0) + 1
                picked.append((f, text))
            if any(os.path.basename(f).startswith("best") for f, _ in picked):
                s["best_at"] = s["reads"]
            self.search_note(s)
            for f, text in picked:
                jid = "w%04d" % self.state["weights_n"]
                seed0 = w["seed0"] + 10000 * self.state["weights_n"]
                self.state["weights_n"] += 1
                what = os.path.basename(f)[:-8]
                moved = short_moved(text)
                jobs.append({"id": jid, "kind": "weights", "title": "%s: %s" % (what, moved or "the defaults"),
                             "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(text, w["seeds"], seed0, jid),
                             "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 6 * 3600})
        return jobs

    # ---- the search's own memory -------------------------------------------------------
    # Trials on today's defaults past the last find (a trial past both its
    # held-out reads): the search widens. After w0034 was promoted the fit's
    # trials stayed at its tops, near what was read, and in 1,000 of them
    # none held out (29 October).
    STALL = 150

    @staticmethod
    def trial_key(text):
        """A trial's weights, three figures each: the fit's top moves by a
        hair as rows come in, and a hair is the same trial."""
        canon = ";".join("%s=%.3g" % kv for kv in sorted(parse_weights(text).items()))
        return hashlib.sha1(canon.encode()).hexdigest()[:12]

    def search(self, defaults):
        """{defaults, tried {key: times planned}, reads, last_find, since_find,
        stalled, bound}: what the search has asked on these defaults (a
        promotion starts it anew), seeded from every trial read, running or
        queued when they are first seen."""
        fp = hashlib.sha1(open(defaults, "rb").read()).hexdigest()[:12]
        s = self.state.get("search")
        trials = [(d, d.rstrip("/").split(os.sep)[-2]) for d in self.census_dirs()]
        trials = [(d, jid) for d, jid in trials if re.match(r"w\d+$", jid) and self.same_defaults(self.census_commit(d))]
        if not s or s.get("defaults") != fp:
            s = {"defaults": fp, "tried": {}, "repeats": 0}
            texts = [open(os.path.join(d, "trial.weights")).read() for d, _ in trials if os.path.exists(os.path.join(d, "trial.weights"))]
            for f in glob.glob(os.path.join(self.q, "queue", "w*.json")) + glob.glob(os.path.join(self.q, "running", "*", "w*.json")):
                m = re.search(r"echo (\S+) \| base64 -d", load(f, {}).get("cmd", ""))
                if m:
                    texts.append(base64.b64decode(m.group(1)).decode())
            for t in texts:
                k = self.trial_key(t)
                s["tried"][k] = s["tried"].get(k, 0) + 1
            self.state["search"] = s
        found = [int(jid[1:]) for jid, ds in self.held_pairs().items()
                 if self.held_pass(ds) and all(self.same_defaults(self.census_commit(d)) for d in ds)]
        last = max(found) if found else None
        since = len([1 for _, jid in trials if last is None or int(jid[1:]) > last])
        s.update({"reads": len(trials), "last_find": "w%04d" % last if last is not None else None,
                  "since_find": since, "stalled": since >= self.STALL})
        s["bound"] = 4 if s["stalled"] else 2
        # Where the widening began, so health can tell widened and still dry.
        if s["stalled"]:
            s.setdefault("widened_at", len(trials))
        else:
            s.pop("widened_at", None)
        return s

    def search_note(self, s):
        """plan/search.json, for fleet-queue.py health and the Settings page."""
        note = {k: v for k, v in s.items() if k != "tried"}
        note.update({"distinct": len(s["tried"]), "stall": self.STALL, "checked": time.strftime("%Y-%m-%dT%H:%M:%S")})
        save(os.path.join(self.pd, "search.json"), note)

    def unit_weights(self):
        """suggest.py's UNIT: weights from 0 to 1, which stop halfway to 1."""
        try:
            m = re.search(r"UNIT = set\('([^']*)'", open(os.path.join(self.tc, "scripts", "suggest.py")).read())
        except OSError:
            m = None
        return set(m.group(1).split()) if m else set()

    def wide_trials(self, n, out, g, defaults):
        """Trials past the fit's bound: three weights at a time, each a
        quarter to four times its default (at least 1.4 times either way),
        the weights the fit is least sure of the likelier. The fit's own
        trials sit at its curves' tops, within what has been read; when they
        stop finding, the room left is outside it."""
        dflt = parse_weights(open(defaults).read())
        try:
            rows = list(csv.DictReader(open(os.path.join(out, "curves.tsv")), delimiter="\t"))
        except OSError:
            return []
        pool = {r["weight"]: float(r["gain_se"]) + 0.1 for r in rows if dflt.get(r["weight"], 0) > 0}
        unit, rnd, files = self.unit_weights(), random.Random(1000003 * g + 7), []
        names = sorted(pool)
        for i in range(n):
            pick = set()
            while len(pick) < min(3, len(names)):
                pick.add(rnd.choices(names, weights=[pool[x] for x in names])[0])
            lines = []
            for w in sorted(pick):
                d = dflt[w]
                y = d * 2 ** (rnd.choice((-1, 1)) * rnd.uniform(0.5, 2.0))
                if w in unit and y > d:
                    y = min(y, d + (1 - d) / 2) if d < 1 else d
                if abs(y - d) > 1e-9:
                    lines.append("%s = %.6g" % (w, y))
            if lines:
                f = os.path.join(out, "wide-%d.weights" % i)
                with open(f, "w") as fh:
                    fh.write("# orrery: wide, past the fit's bound (the search stalled)\n" + "\n".join(lines) + "\n")
                files.append(f)
        return files

    # ---- the sequential read -----------------------------------------------------------
    # Fishtest's way, in seeds: a test begun is read on until it is decided,
    # pooled. At 55 seeds a census is good to +/-0.66 a seat (a seed swings
    # 4.9), so a trial truly +0.5 above passed 11 times in 100, and a trial
    # no better passed 2 in 100: of 1,000 trials after w0034 every one that
    # passed was noise, and each cost two held-out reads of 400 seeds. Each
    # stage: (seeds read by then, read on at z past, pass at z past). Against
    # one fixed read of 55, simulated with the measured noise and a fleet of
    # mostly no-better trials: 40% more real finds a seed, a quarter of the
    # false passes (29 October). The held-out reads still decide.
    SEQ = ((1, 0.5, None), (2, 1.0, None), (4, 1.5, 2.6), (8, None, 2.0))

    # The held-out reads decide together: pooled, 800 seeds, z past HELD_Z and
    # no map two errors under. Each read had to pass alone (d past two of its
    # errors, about +0.55), and a trial truly +0.6 did that both times one
    # time in three: w1198 (+0.67 +/- 0.19 pooled, z 3.5) and w1266 (+0.59 +/-
    # 0.16, z 3.7) each passed one read and were lost (29 October). A trial no
    # better reaches z 3 one time in 750, near the old rule's one in 2,000; a
    # trial +0.6 does six times in ten.
    HELD_Z = 3.0

    def held_pass(self, ds):
        """True when a trial's two held-out reads, pooled, pass."""
        p = self.pool(ds) if len(ds) == 2 else None
        return bool(p) and p["every"][1] > 0 and p["every"][0] / p["every"][1] >= self.HELD_Z and not p["below"]

    @staticmethod
    def pool(dirs):
        """The censuses of one trial pooled, by inverse variance: {"every":
        (d, se, seeds), "below": a map more than two errors under}."""
        every, maps = [], {}
        for d in dirs:
            try:
                lines = open(os.path.join(d, "pooled.txt")).read().splitlines()
            except OSError:
                continue
            for l in lines:
                m = re.match(r"^(\S+(?: map)?)\s+([-+][\d.]+) \+/- ([\d.]+)(?: over\s+(\d+) seeds)?", l)
                if not m or float(m.group(3)) <= 0:
                    continue
                row = (float(m.group(2)), float(m.group(3)), int(m.group(4) or 0))
                (every.append(row) if m.group(1) == "every map" else maps.setdefault(m.group(1), []).append(row))

        def comb(rows):
            iw = sum(1 / se ** 2 for _, se, _ in rows)
            return sum(d / se ** 2 for d, se, _ in rows) / iw, 1 / math.sqrt(iw), sum(n for _, _, n in rows)
        if not every:
            return None
        below = any(dd < -2 * se for dd, se, _ in (comb(r) for r in maps.values()))
        return {"every": comb(every), "below": below}

    def sequential(self):
        """{trial: {status, d, se, seeds, z, stage, text}} for every trial on
        today's defaults: reading (a read of it queued or running), read on
        (its stage says more), passed, or dropped. Its reads are its own
        census and the jobs titled "read on N of TRIAL"."""
        title, inflight = {}, set()
        for f in glob.glob(os.path.join(self.q, "done", "*.json")):
            m = re.match(r"read on \d+ of (w\d+)", load(f, {}).get("title", ""))
            if m:
                title[load(f, {})["id"]] = m.group(1)
        for f in glob.glob(os.path.join(self.q, "queue", "*.json")) + glob.glob(os.path.join(self.q, "running", "*", "*.json")):
            j = load(f, {})
            m = re.match(r"read on \d+ of (w\d+)", j.get("title", ""))
            if m or re.match(r"w\d+$", j.get("id", "")):
                inflight.add(m.group(1) if m else j["id"])
        reads = {}
        for d in self.census_dirs():
            jid = d.rstrip("/").split(os.sep)[-2]
            if re.match(r"w\d+$", jid):
                reads.setdefault(jid, []).insert(0, d)
            elif jid in title:
                reads.setdefault(title[jid], []).append(d)
        base = self.cfg["weights"]["seeds"]
        out = {}
        for jid, ds in reads.items():
            if not os.path.exists(os.path.join(ds[0], "trial.weights")) or not self.same_defaults(self.census_commit(ds[0])):
                continue
            p = self.pool(ds)
            if not p:
                continue
            d, se, n = p["every"]
            z = d / se
            stage = max([i for i, s in enumerate(self.SEQ) if s[0] * base <= n] or [0])
            _, keep, ok = self.SEQ[stage]
            if jid in self.state["held_read"]:
                status = "passed"
            elif jid in inflight:
                status = "reading"
            elif ok is not None and z >= ok and not p["below"]:
                status = "passed"
            elif keep is None or z < keep:
                # A map under blocks a pass, not a read on: eleven maps at 55
                # seeds put one two errors under, by chance, a time in five.
                status = "dropped"
            else:
                status = "read on"
            out[jid] = {"status": status, "d": d, "se": se, "seeds": n, "z": z, "stage": stage,
                        "text": open(os.path.join(ds[0], "trial.weights")).read()}
        return out

    def read_on_jobs(self, seq, room, w):
        """The next stage's census for the trials that read on, highest z
        first: as many seeds again as read so far, on fresh seeds (300M up,
        clear of the trials', the held-out reads' and the rotation's)."""
        jobs = []
        for jid, v in sorted(seq.items(), key=lambda kv: -kv[1]["z"]):
            if len(jobs) >= room:
                break
            if v["status"] != "read on":
                continue
            base = w["seeds"]
            more = self.SEQ[v["stage"] + 1][0] * base - v["seeds"]
            if more <= 0:
                continue
            k = self.state.get("read_on_n", 0)
            self.state["read_on_n"] = k + 1
            eid = "e%04d" % k
            jobs.append({"id": eid, "kind": "weights",
                         "title": "read on %d of %s (%d seeds more, z %+.2f on %d)" % (v["stage"] + 2, jid, more, v["z"], v["seeds"]),
                         "needs": "cpu", "after_host": self.after_host(),
                         "cmd": self.census_cmd(v["text"], more, 300000000 + 1000 * k, eid),
                         "fetch": "%s/out/%s" % (self.home, eid), "timeout_s": 6 * 3600})
        return jobs

    @staticmethod
    def seq_counts(seq):
        """For plan/search.json: how many trials at each status, and the
        seeds a decided trial took."""
        out = {}
        for v in seq.values():
            out[v["status"]] = out.get(v["status"], 0) + 1
        decided = [v["seeds"] for v in seq.values() if v["status"] in ("passed", "dropped")]
        out["seeds_a_decision"] = round(sum(decided) / len(decided)) if decided else None
        return out

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
                if self.held_pass(ds):
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
            "SEEDS=%d SEED0=%d SECS=%d MAP=%s RESPAWNS=3 JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) CENSUS_NET=$Q/judge/%03d/%%s.bin CENSUS_BIN=$C/census-bin scripts/census.sh 2>&1 | tail -2\n"
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
        ps = self.push_check() if not only else load(os.path.join(self.pd, "push.json"), {})
        summary = {"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "rows": sum(by.values()), "by_host": by, "hosts": lines, "push": ps}
        save(os.path.join(out, "sync.json"), summary)
        print("sync: fleet.tsv %d rows (%s); read: %s" % (summary["rows"], ", ".join("%s %d" % kv for kv in sorted(by.items())), "; ".join(lines)))


    # ---- runs: what a person asks for (fleet-queue.py Q new KIND k=v ...) -------------
    # A run is a request in Q/runs/RID.json ({"kind", "params"}); its state and
    # results are in plan/runs/RID/. The kinds are what treadcore's make
    # targets were (LR 114):
    #   census       one census: trial against base (TRIAL=, BASE= files or text)
    #   parts        each part off (scripts/parts.sh LIST=1), a census each
    #   switches     each switch alone (scripts/switches.sh LIST=1), a census each
    #   roundrobin   every ordered pair of profiles (scripts/roundrobin.sh LIST=1)
    #   spsa         SPSA over named weights, a step a census (scripts/spsa.sh's rule)
    #   tune         a (1 + lambda) evolution strategy, a child a census (tune.sh's rule)
    #   train, gauntlet, clips, school   the learning harness, one job on one host
    #   record       recordings for behaviour cloning, a shard a job, to the store
    #   families     every candidate's families read again
    #   promote      a candidate written into weights.rs on a branch, tested,
    #                its families read, committed and pushed as the branch;
    #                main moves only when Paul approves (approve RID)
    # Every census is one machine's; a run's censuses go to whichever is free.
    CENSUS_KINDS = ("census", "parts", "switches", "roundrobin")
    HARNESS_KINDS = ("train", "gauntlet", "clips", "school")

    def runs(self):
        out = []
        for f in sorted(glob.glob(os.path.join(self.q, "runs", "*.json"))):
            r = load(f)
            if r and r.get("kind"):
                r["id"] = os.path.basename(f)[:-5]
                out.append(r)
        return out

    def rdir(self, rid):
        d = os.path.join(self.pd, "runs", rid)
        os.makedirs(d, exist_ok=True)
        return d

    def rstate(self, rid):
        if rid in self._rsaves:
            return self._rsaves[rid]
        return load(os.path.join(self.rdir(rid), "state.json"), {"status": "new"})

    def rsave(self, rid, st):
        # Kept until the top-up succeeds (keep()): a run is never marked as
        # having jobs the keeper did not receive.
        self._rsaves[rid] = st
        # A run that ends, or waits for a person, is a flag.
        status = st.get("status")
        if status in ("done", "failed", "prepared"):
            text = {"done": "run %s finished" % rid, "failed": "run %s failed: %s" % (rid, st.get("why", "")),
                    "prepared": "promotion %s is prepared on %s: Paul Accepts it when auto mode is on and this machine can push; "
                                "otherwise it self-modifies (the local line)" % (rid, st.get("branch"))}[status]
            # News is progress (Paul, 28 October: "I'm going to always accept
            # them"); only a failure asks for a person. A prepared promotion
            # never waits: it is accepted or it self-modifies.
            asks = status == "failed"
            self.flag("run" if status != "prepared" else "approve", text, key="run:%s:%s" % (rid, status), progress=not asks)

    # ---- auto mode: the cycle closes itself ---------------------------------------------
    def auto(self):
        """Auto mode (config "auto", on unless false): a run's result gets its
        held-out reads, the strongest current candidate promotes itself, and a
        prepared promotion approves itself (Paul, 28 October: "Should self
        promote, unless we are not in AUTO mode. We should be in auto mode")."""
        return self.cfg.get("auto", True) is not False

    def new_run(self, kind, params, by, **extra):
        """A run file, as fleet-queue.py new writes one."""
        d = os.path.join(self.q, "runs")
        os.makedirs(d, exist_ok=True)
        n = 1 + max([int(f[1:5]) for f in os.listdir(d) if f[:1] == "r" and f[1:5].isdigit()] +
                    [int(r[1:]) for r, _ in self._newruns] or [0])
        rid = "r%04d" % n
        # Written by keep(), with the state: a pass that fails leaves no run
        # behind it (28 October: one did, and its state was not saved).
        self._newruns.append((rid, dict({"kind": kind, "params": params, "asked": time.strftime("%Y-%m-%dT%H:%M:%S"), "by": by}, **extra)))
        return rid

    def adopt(self, text, what):
        """A result (a run's weights) as a trial of the queue: its census and
        its two held-out reads, queued together. Passing both, it is a
        candidate like any other. Returns the trial's id."""
        self.state.setdefault("adopt", []).append({"text": text, "what": what})
        return "w%04d" % (self.state["weights_n"] + len(self.state["adopt"]) - 1)

    def adopt_runs(self):
        """Each finished spsa or tune run's result, adopted once (auto mode)."""
        if not self.auto() or not self.defaults():
            return
        done = self.state.setdefault("adopted", {})
        for r in self.runs():
            rid, kind = r["id"], r["kind"]
            st = self.rstate(rid)
            if kind not in ("spsa", "tune", "census") or rid in done or st.get("status") != "done":
                continue
            if kind == "census":
                # A census of a trial (the rotation's @best): adopted when it
                # passed, so its held-out reads follow.
                items = st.get("items", [])
                ds = glob.glob(os.path.join(self.q, "results", items[0]["job"], self.home, "out", items[0]["job"], "*", "")) if len(items) == 1 else []
                if not ds or not self.passes(ds[0]) or not os.path.exists(os.path.join(ds[0], "trial.weights")):
                    done[rid] = None
                    continue
                text = open(os.path.join(ds[0], "trial.weights")).read()
            else:
                f = os.path.join(self.rdir(rid), "theta.weights" if kind == "spsa" else "best.weights")
                text = open(f).read() if os.path.exists(f) else ""
            dflt = parse_weights(open(self.defaults()).read()) if self.defaults() else {}
            moved = {k: v for k, v in parse_weights(text).items() if k in dflt and abs(v - dflt[k]) > 1e-9}
            if not moved:
                done[rid] = None
                continue
            done[rid] = self.adopt(text, "%s %s" % (kind, rid))

    # One of each, in turn: the default work of auto mode beside the standing
    # search (Paul, 28 October: "Is there a do one of each on rotation method
    # that is the default choice? Can we have that."). Two lanes, a run
    # active in each: the brain's searches and measurements, and the
    # learned tank's (train and school can take days).
    ROTATION = {
        "brain": [["spsa", {}], ["tune", {}], ["census", {"trial": "@best", "name": "best"}],
                  ["parts", {}], ["switches", {}], ["roundrobin", {}], ["record", {}]],
        "net": [["gauntlet", {}], ["train", {}], ["school", {}]],
    }
    # Measurements that say nothing new on the same defaults: once each.
    ONCE_PER_DEFAULTS = ("parts", "switches", "roundrobin", "gauntlet")

    def rotate(self):
        if not self.auto() or self.cfg.get("rotation") is False:
            return
        rot = self.state.setdefault("rotation", {})
        runs = self.runs()
        lanes = self.cfg.get("rotation") if isinstance(self.cfg.get("rotation"), dict) else self.ROTATION
        for lane, kinds in lanes.items():
            if any(r.get("lane") == lane and self.rstate(r["id"]).get("status") in ("new", "running") for r in runs):
                continue
            i = rot.get(lane, 0)
            for step in range(len(kinds)):
                kind, params = kinds[(i + step) % len(kinds)]
                if kind in self.ONCE_PER_DEFAULTS and any(
                        r["kind"] == kind and self.rstate(r["id"]).get("status") in ("done", "running")
                        and self.same_defaults(self.rstate(r["id"]).get("commit")) for r in runs):
                    continue
                params = dict(params)
                cycle = rot.get("cycle", 0)
                rot["cycle"] = cycle + 1
                # Fresh seeds every time, clear of the trials' (20M), the
                # held-out reads' (60M up) and the recordings' (9.9M).
                if kind in ("spsa", "tune", "census"):
                    params["seed0"] = 500000000 + 1000000 * cycle
                if kind == "record":
                    nxt = rot.get("record_next", 9920000)
                    params.update({"seed0": nxt, "count": 48, "per": 12, "run": "rot%d" % cycle})
                    rot["record_next"] = nxt + 48
                rid = self.new_run(kind, params, "rotation", lane=lane)
                rot[lane] = (i + step + 1) % len(kinds)
                self.flag("rotation", "rotation (%s): %s asked a %s" % (lane, rid, kind), key="rotation:%s" % rid, progress=True)
                break

    def adopt_jobs(self):
        w = self.cfg.get("weights")
        if not w or not self.state.get("adopt") or not self.cpu_ready():
            return []
        jobs = []
        for a in self.state["adopt"]:
            jid = "w%04d" % self.state["weights_n"]
            self.state["weights_n"] += 1
            seed0 = w["seed0"] + 10000 * int(jid[1:])
            jobs.append({"id": jid, "kind": "weights", "title": "%s: %s" % (a["what"], short_moved(a["text"])),
                         "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(a["text"], w["seeds"], seed0, jid),
                         "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 6 * 3600})
            # Its held-out reads at once, not after its own census passes:
            # a run's result was already chosen by a search.
            for k in range(2):
                hid = "h%04d" % self.state["held_n"]
                hseed = w["held_seed0"] + 200000 * self.state["held_n"]
                self.state["held_n"] += 1
                jobs.append({"id": hid, "kind": "weights", "title": "held-out read %d of %s (%d seeds)" % (k + 1, jid, w["held_seeds"]),
                             "needs": "cpu", "after_host": self.after_host(), "cmd": self.census_cmd(a["text"], w["held_seeds"], hseed, hid),
                             "fetch": "%s/out/%s" % (self.home, hid), "timeout_s": 6 * 3600})
            self.state["held_read"].append(jid)
        self.state["adopt"] = []
        return jobs

    @staticmethod
    def census_commit(d):
        try:
            for l in open(os.path.join(d, "run.env")):
                if l.startswith("COMMIT="):
                    return l.split("=", 1)[1].strip().strip("'")
        except OSError:
            pass
        return None

    def auto_promote(self):
        """The strongest current candidate whose families hold, promoted: one
        promotion at a time, each candidate tried once."""
        if not self.auto():
            return
        autos = [r for r in self.runs() if r["kind"] == "promote" and r.get("by") == "auto"]
        for r in self.runs():
            if r["kind"] == "promote" and self.rstate(r["id"]).get("status") in ("new", "running", "prepared"):
                return
        # Two failed in a row: something every candidate trips on (a test, a
        # family). Auto mode waits for a person rather than trying them all.
        last = [self.rstate(r["id"]).get("status") for r in autos[-2:]]
        if len(last) == 2 and all(x == "failed" for x in last):
            self.flag("auto", "auto promotion paused: %s and %s failed in a row; see their logs, then ask for a promotion by hand"
                      % (autos[-2]["id"], autos[-1]["id"]), key="paused:%s" % autos[-1]["id"])
            return
        tried = self.state.setdefault("auto_tried", [])
        ev = {k: v for k, v in self.evidence().items()
              if v.get("current") and v["families"] == "holds" and v["held_mean"] is not None and k not in tried}
        if not ev:
            return
        name = max(ev, key=lambda k: ev[k]["held_mean"])
        tried.append(name)
        rid = self.new_run("promote", {"candidate": name}, "auto")
        self.flag("promote", "auto: %s asked to promote %s (held out %s)" % (rid, name, " and ".join("%+.2f" % h[0] for h in ev[name]["held"])),
                  key="auto:%s" % name, progress=True)

    # ---- older candidates, read again on today's defaults ----------------------------
    # Paul, 28 October: "why are some older, when it says promoted". A
    # candidate is read against the defaults of its day; a promotion changes
    # the defaults, and every other candidate is then evidence about a brain
    # that is no longer the game's. It may promote itself only when read
    # again on today's. With no current candidate, auto mode re-reads the
    # strongest older ones (REREAD a time): each a new trial with its census
    # and two held-out reads, on the newest commit.
    REREAD = 3

    def defaults_blob(self):
        return self.state["commits"].get(self.sha(), {}).get("weights_rs") or self.weights_rs(self.sha())

    def reread_older(self, force=False):
        """Queue the strongest older candidates again; returns their names."""
        if not force and not self.auto():
            return []
        ev = self.evidence()
        if not force and any(v.get("current") for v in ev.values()):
            return []
        done = self.state.setdefault("reread", {})
        blob = self.defaults_blob()
        # One round a set of defaults, unless asked.
        if not force and any(b == blob for b in done.values()):
            return []
        # A promoted candidate is the defaults: read again it would be
        # measured against itself.
        promoted = {self.rstate(r["id"]).get("candidate") for r in self.runs()
                    if r["kind"] == "promote" and self.rstate(r["id"]).get("status") in ("approved", "self-modified")}
        older = sorted(((k, v) for k, v in ev.items() if k.startswith(self.name + "/") and not v.get("current")
                        and k not in promoted and v["held_mean"] is not None and done.get(k) != blob),
                       key=lambda kv: -kv[1]["held_mean"])[:self.REREAD]
        for k, v in older:
            self.adopt(v["weights"], "%s again on %s" % (k.split("/", 1)[1], self.short()))
            done[k] = blob
            self.flag("reread", "%s read again on today's defaults (%s): it was read on older ones" % (k, self.short()),
                      key="reread:%s:%s" % (k, blob[:8]), progress=True)
        return [k for k, _ in older]

    def now(self):
        """Do it now (Paul, 28 October: "it has a do it now button to sync and
        push the values"): sync, then the strongest current candidate whose
        families hold is promoted (tests and families on a branch, then
        main); with none, the strongest older ones are read again."""
        self.sync()
        for r in self.runs():
            if r["kind"] == "promote" and self.rstate(r["id"]).get("status") in ("new", "running", "prepared"):
                print("a promotion is already under way: %s" % r["id"])
                return
        ev = {k: v for k, v in self.evidence().items() if v.get("current") and v["families"] == "holds" and v["held_mean"] is not None}
        if ev:
            name = max(ev, key=lambda k: ev[k]["held_mean"])
            rid = self.new_run("promote", {"candidate": name}, "now")
            self.keep()
            print("%s: promoting %s (held out %s); in auto mode it is pushed to main when its tests and families pass"
                  % (rid, name, " and ".join("%+.2f" % h[0] for h in ev[name]["held"])))
            return
        names = self.reread_older(force=True)
        self.keep()
        print("no candidate on today's defaults (%s) to promote; read again: %s" % (self.short(), ", ".join(names) or "none left"))

    def propagation(self):
        """For each approved promotion: has each machine built a commit whose
        defaults carry the promoted values? Read from the defaults.toml each
        machine's build brought back; confirmed on all, it is progress."""
        for r in self.runs():
            rid = r["id"]
            if r["kind"] != "promote":
                continue
            st = self.rstate(rid)
            if st.get("status") not in ("approved", "self-modified") or st.get("confirmed"):
                continue
            want = {k: v["value"] for k, v in changed_vars(st.get("changed", [])).items()}
            # Every commit main has been at since the promotion (each carries
            # its values, unless a later promotion moved them): a machine is
            # confirmed by any finished build of one of them whose defaults
            # have the values, not only by its newest, which may still be
            # queued behind its work (28 October: two of seven read
            # "confirmed" while all seven had built them).
            since = st.get("approved", "")
            shorts = [st.get("commit", "")[:7]] + [c[:7] for c, v in self.state.get("commits", {}).items()
                                                   if v.get("first_seen", "") >= since and c[:7] != st.get("commit", "")[:7]]
            done, live, gone = self.ids("done"), self.ids("queue", "running"), self.ids("failed", "stopped")
            hosts = {}
            for h in self.hosts:
                name, ok, differs, building, failed = h["name"], None, None, None, None
                for short in shorts:
                    jid = "prep-%s-%s" % (name, short)
                    if jid in done:
                        f = glob.glob(os.path.join(self.q, "results", jid, self.home, "c", short, "defaults.toml"))
                        if not f:
                            continue
                        got = parse_weights(open(f[0]).read())
                        off = [k for k, v in want.items() if k not in got or abs(got[k] - v) > 1e-6 * max(1.0, abs(v))]
                        if off:
                            differs = differs or "differs at %s: %s" % (short, ", ".join(off))
                        else:
                            ok = ok or short
                    elif jid in live:
                        building = building or short
                    elif jid in gone:
                        failed = failed or short
                hosts[name] = ("confirmed at %s" % ok if ok else differs if differs else "building %s" % building if building
                               else "build of %s failed" % failed if failed else "waiting for its build")
            st["hosts"] = hosts
            ok = [n for n, v in hosts.items() if v.startswith("confirmed")]
            st["detail"] = "on main at %s; values confirmed on %d of %d machines%s" % (
                st.get("commit", "")[:7], len(ok), len(hosts),
                "" if len(ok) == len(hosts) else " (" + "; ".join("%s: %s" % (n, v) for n, v in hosts.items() if n not in ok) + ")")
            if hosts and len(ok) == len(hosts):
                st["confirmed"] = time.strftime("%Y-%m-%dT%H:%M:%S")
                self.flag("propagated", "promotion %s: %s's values confirmed on all %d machines (%s)" % (
                    rid, st.get("candidate"), len(hosts), ", ".join(sorted(hosts))), key="propagated:%s" % rid, progress=True)
            elif any(v.startswith("differs") or "failed" in v for v in hosts.values()):
                self.flag("propagated", "promotion %s: not every machine has its values: %s" % (rid, st["detail"]), key="propagation:%s:%s" % (rid, st["detail"]))
            self.rsave(rid, st)

    def text(self, v):
        """A weights parameter: a file under treadcore, or the text itself ("a = 1;b = 2")."""
        if not v:
            return ""
        if v == "@best":
            # The newest best the queue's search suggested.
            gens = sorted(glob.glob(os.path.join(self.pd, "gen*", "best.weights")), key=lambda f: int(re.search(r"gen(\d+)", f).group(1)))
            return open(gens[-1]).read() if gens else ""
        p = os.path.join(self.tc, v) if not os.path.isabs(v) else v
        if os.path.isfile(p):
            return open(p).read()
        return v.replace(";", "\n") + "\n"

    def census_job(self, jid, title, trial, base, seeds, seed0, secs, mp, respawns=3, net=None, host=None):
        b1 = base64.b64encode(trial.encode()).decode()
        b2 = base64.b64encode(base.encode()).decode()
        net_env = ""
        send = []
        if net:
            send = [{"src": net, "dst": "~/%s/trials/%s.net" % (self.home, jid)}]
            net_env = "CENSUS_NET=$Q/trials/%s.net " % jid
        cmd = (
            "set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:$PATH\n"
            "Q=~/%s; C=%s; mkdir -p $Q/trials\n"
            "echo %s | base64 -d > $Q/trials/%s.trial; echo %s | base64 -d > $Q/trials/%s.base\n"
            "cd $C/wt && export CARGO_TARGET_DIR=$HOME/fleet/target\n"
            "%s%s%sSEEDS=%d SEED0=%d SECS=%s MAP=%s RESPAWNS=%s JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) CENSUS_BIN=$C/census-bin scripts/census.sh 2>&1 | tail -3\n"
            "d=$(ls -td build/census/*/ | head -1); s=$(basename $d)\n"
            "rm -rf $Q/out/%s; mkdir -p $Q/out/%s/$s; cp $d/*.txt $d/*.env $d/*.weights $d/g-*.log $Q/out/%s/$s/ 2>/dev/null; rm -rf $d\n"
        ) % (self.home, self.cdir(), b1, jid, b2, jid,
             "TRIAL=$Q/trials/%s.trial " % jid if trial.strip() else "",
             "BASE=$Q/trials/%s.base " % jid if base.strip() else "", net_env,
             int(seeds), int(seed0), secs, mp, respawns, jid, jid, jid)
        j = {"id": jid, "kind": "census", "title": title, "needs": "host:" + host if host else "cpu",
             "after_host": self.after_host(), "cmd": cmd, "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 12 * 3600}
        if send:
            j["send"] = send
        return j

    def job_census(self, jid):
        """(d, se, verdict line, host) of a fetched census job, or None."""
        ds = glob.glob(os.path.join(self.q, "results", jid, self.home, "out", jid, "*", "pooled.txt"))
        if not ds:
            return None
        lines = open(ds[0]).read().splitlines()
        for l in lines:
            m = re.match(r"^every map\s+([-+][\d.]+) \+/- ([\d.]+) over\s+(\d+) seeds \((.*)\)", l)
            if m:
                envs = env_file(os.path.join(os.path.dirname(ds[0]), "run.env"))
                host = self.qcfg.get("aliases", {}).get(envs.get("HOST", ""), envs.get("HOST", ""))
                return float(m.group(1)), float(m.group(2)), m.group(4), host
        return None

    def list_items(self, kind, params):
        """The censuses of parts, switches or roundrobin, from treadcore's LIST=1."""
        env = dict(os.environ, LIST="1")
        for k, v in params.items():
            env[k.upper()] = str(v)
        r = run(["bash", os.path.join(self.tc, "scripts", "%s.sh" % kind)], env=env, cwd=self.tc, timeout=60)
        if r.returncode != 0:
            raise SystemExit("%s LIST=1: %s" % (kind, r.stderr[-300:]))
        settings, items = {}, []
        for l in r.stdout.splitlines():
            if l.startswith("#settings"):
                settings = dict(kv.split("=", 1) for kv in l.split()[1:])
            elif l.strip():
                name, trial, base = (l.split("\t") + ["", ""])[:3]
                items.append({"name": name, "trial": trial.replace(";", "\n") + "\n", "base": base.replace(";", "\n") + "\n"})
        return settings, items

    def run_jobs(self, n):
        # Runs advance whether or not a machine has built the newest commit:
        # a new job waits for its machine's build (after_host) by itself.
        jobs = []
        for r in self.runs():
            if len(jobs) >= n:
                break
            first = len(jobs)
            rid, kind, p = r["id"], r["kind"], r.get("params", {})
            st = self.rstate(rid)
            if st["status"] in ("done", "failed", "stopped", "approved", "self-modified", "rejected"):
                continue
            if kind in self.CENSUS_KINDS:
                jobs += self.census_run(rid, kind, p, st)
            elif kind == "spsa":
                jobs += self.spsa_run(rid, p, st)
            elif kind == "tune":
                jobs += self.tune_run(rid, p, st)
            elif kind in self.HARNESS_KINDS:
                jobs += self.harness_run(rid, kind, p, st)
            elif kind == "record":
                jobs += self.record_run(rid, p, st, n - len(jobs))
            elif kind == "promote":
                jobs += self.promote_run(rid, p, st)
            elif kind == "families":
                # Every candidate's families read again (after a promotion,
                # say): the next top-ups queue them all.
                self.state["families_queued"] = []
                self.rsave(rid, {"status": "done", "detail": "every candidate queued again"})
            else:
                st.update({"status": "failed", "why": "no such kind %s" % kind})
                self.rsave(rid, st)
            # What a person asked for goes ahead of the standing search; the
            # rotation's runs share the fleet with it.
            if r.get("by") != "rotation":
                for j in jobs[first:]:
                    j["priority"] = "high"
        return jobs

    # census, parts, switches, roundrobin: a census an item, then a table.
    def census_run(self, rid, kind, p, st):
        if st["status"] == "new":
            if kind == "census":
                settings = {"SEEDS": p.get("seeds", 55), "SEED0": p.get("seed0", 0), "SECS": p.get("secs", 900),
                            "MAP": p.get("map", "all"), "RESPAWNS": p.get("respawns", 3)}
                items = [{"name": p.get("name", "trial"), "trial": self.text(p.get("trial", "")), "base": self.text(p.get("base", ""))}]
            else:
                settings, items = self.list_items(kind, {k: v for k, v in p.items() if k.upper() in
                                                         ("PARTS", "SWITCHES", "PROFILES", "SEEDS", "SEED0", "SECS", "MAP", "RESPAWNS")})
            st = {"status": "running", "settings": settings, "items": [], "commit": self.short()}
            jobs = []
            for i, it in enumerate(items):
                jid = "%s-%02d" % (rid, i)
                st["items"].append({"name": it["name"], "job": jid})
                jobs.append(self.census_job(jid, "%s %s: %s" % (kind, rid, it["name"]), it["trial"], it["base"],
                                            settings["SEEDS"], settings.get("SEED0", 0), settings["SECS"], settings["MAP"],
                                            settings.get("RESPAWNS", 3), net=p.get("net")))
            self.rsave(rid, st)
            return jobs
        # Running: the table, when every census is back.
        done = self.ids("done")
        gone = self.ids("failed", "stopped")
        rows, left = [], 0
        for it in st["items"]:
            c = self.job_census(it["job"]) if it["job"] in done else None
            if c:
                rows.append("%-28s %+8.2f %6.2f  %-22s %s" % (it["name"], c[0], c[1], c[2], c[3]))
            elif it["job"] in gone:
                rows.append("%-28s %8s %6s  %s" % (it["name"], "?", "?", "failed"))
            else:
                left += 1
        if left == 0:
            with open(os.path.join(self.rdir(rid), "table.txt"), "w") as f:
                f.write("%s %s at %s: %s\n" % (kind, rid, st.get("commit"), " ".join("%s=%s" % kv for kv in st["settings"].items())))
                f.write("%-28s %8s %6s  %-22s %s\n" % ("name", "diff", "se", "verdict", "machine"))
                f.write("\n".join(rows) + "\n")
            st["status"] = "done"
            self.rsave(rid, st)
        return []

    # SPSA (scripts/spsa.sh's rule, a step a census on one machine).
    def theta0(self, p):
        if p.get("start"):
            return parse_weights(self.text(p["start"]))
        return parse_weights(open(self.defaults()).read())

    def spsa_run(self, rid, p, st):
        tune = p.get("tune", "dodge_horizon dodge_margin dodge_clear").replace(",", " ").split()
        steps, seeds, seed0 = int(p.get("steps", 8)), int(p.get("seeds", 12)), int(p.get("seed0", 1000))
        A, C, maxstep, bound = float(p.get("a", 0.6)), float(p.get("c", 0.4)), float(p.get("maxstep", 0.5)), float(p.get("bound", 2))
        if st["status"] == "new":
            if not p.get("start") and not self.defaults():
                return []  # the census host has not built this commit yet
            th = self.theta0(p)
            for w in tune:
                if w not in th or th[w] <= 0:
                    st.update({"status": "failed", "why": "%s is not a positive weight" % w})
                    self.rsave(rid, st)
                    return []
            st = {"status": "running", "k": 0, "theta": {w: th[w] for w in tune}, "start": {w: th[w] for w in tune},
                  "steps": [], "pending": None, "commit": self.short()}
        if st.get("pending"):
            pend = st["pending"]
            if pend["job"] in self.ids("failed", "stopped"):
                st.update({"status": "failed", "why": "step %d's census failed" % pend["k"]})
                self.rsave(rid, st)
                return []
            c = self.job_census(pend["job"]) if pend["job"] in self.ids("done") else None
            if not c:
                return []
            d, se = c[0], c[1]
            k, ak, ck = pend["k"], pend["ak"], pend["ck"]
            row = {"k": k, "d": d, "se": se, "host": c[3]}
            for w in tune:
                sgn = pend["sign"][w]
                step = ak * d / (2 * ck * sgn)
                step = max(-maxstep, min(maxstep, step))
                x = st["theta"][w] * math.exp(step)
                if bound > 0:
                    x = max(st["start"][w] / bound, min(st["start"][w] * bound, x))
                st["theta"][w] = round(x, 4)
                row[w] = st["theta"][w]
            st["steps"].append(row)
            st["pending"] = None
            self.tick({"kind": "spsa", "run": rid, "step": k, "d": d, "se": se, "host": c[3],
                       "vars": {w: st["theta"][w] for w in tune}, "improved": False})
            write_tsv(os.path.join(self.rdir(rid), "steps.tsv"), st["steps"])
            with open(os.path.join(self.rdir(rid), "theta.weights"), "w") as f:
                f.write("".join("%s = %s\n" % kv for kv in st["theta"].items()))
        if st["k"] >= steps:
            st["status"] = "done"
            self.rsave(rid, st)
            return []
        k = st["k"] + 1
        ak, ck = A / (k + 1) ** 0.602, C / k ** 0.101
        rnd = random.Random("%s-%d" % (rid, k))
        sign = {w: rnd.choice((-1, 1)) for w in tune}
        plus = "".join("%s = %.4f\n" % (w, st["theta"][w] * math.exp(ck * sign[w])) for w in tune)
        minus = "".join("%s = %.4f\n" % (w, st["theta"][w] * math.exp(-ck * sign[w])) for w in tune)
        base_extra = self.text(p.get("start", "")) if p.get("start") else ""
        jid = "%s-k%02d" % (rid, k)
        st["k"] = k
        st["pending"] = {"k": k, "ak": ak, "ck": ck, "sign": sign, "job": jid}
        self.rsave(rid, st)
        return [self.census_job(jid, "spsa %s: step %d of %d" % (rid, k, steps), base_extra + plus, base_extra + minus,
                                seeds, seed0 + (k - 1) * seeds, p.get("secs", 900), p.get("map", "maps/naval.map"))]

    # A (1 + lambda) evolution strategy (scripts/tune.sh's rule), a
    # generation's children side by side, the best replacing the parent
    # only by beating it.
    def tune_run(self, rid, p, st):
        default_tune = "w_near w_wounded w_finish w_focus w_overkill w_in_range w_threat w_stick engage_odds press_odds w_exposure w_flank k_long c_boat swap_gap"
        tune = p.get("tune", default_tune).replace(",", " ").split()
        gens, lam, sigma = int(p.get("generations", 5)), int(p.get("lambda", 4)), float(p.get("sigma", 0.15))
        seeds, secs, mp = int(p.get("seeds", 2)), p.get("secs", 900), p.get("map", "all")
        # Its seeds: the same for every child of a run (a fair comparison),
        # new for each run (the rotation passes seed0).
        seed0 = int(p.get("seed0", 0))
        done, gone = self.ids("done"), self.ids("failed", "stopped")
        if st["status"] == "new":
            if not p.get("start") and not self.defaults():
                return []  # the census host has not built this commit yet
            th = self.theta0(p)
            parent = {w: th[w] for w in tune if w in th}
            jid = "%s-g00-parent" % rid
            st = {"status": "running", "g": 0, "parent": parent, "fit": None, "children": [jid], "kids": {jid: parent},
                  "log": [], "commit": self.short()}
            self.rsave(rid, st)
            return [self.census_job(jid, "tune %s: the parent" % rid, "".join("%s = %s\n" % kv for kv in parent.items()), "",
                                    seeds, seed0, secs, mp)]
        if any(j in gone for j in st["children"]):
            st.update({"status": "failed", "why": "a child's census failed"})
            self.rsave(rid, st)
            return []
        if not all(j in done for j in st["children"]):
            return []
        fits = {j: self.job_census(j) for j in st["children"]}
        for j, c in fits.items():
            if c is None:
                continue
            better = st["fit"] is not None and c[0] > st["fit"]
            if st["fit"] is None:
                st["fit"] = c[0]
            elif better:
                st["parent"], st["fit"] = st["kids"][j], c[0]
            st["log"].append({"g": st["g"], "job": j, "fit": c[0], "se": c[1], "host": c[3], "parent": better})
            if st["g"] > 0:
                self.tick({"kind": "tune", "run": rid, "job": j, "d": c[0], "se": c[1], "host": c[3],
                           "vars": st["kids"][j], "improved": better})
        write_tsv(os.path.join(self.rdir(rid), "children.tsv"), st["log"])
        with open(os.path.join(self.rdir(rid), "best.weights"), "w") as f:
            f.write("# tune %s: fitness %s\n" % (rid, st["fit"]) + "".join("%s = %s\n" % kv for kv in st["parent"].items()))
        if st["g"] >= gens:
            st["status"] = "done"
            self.rsave(rid, st)
            return []
        st["g"] += 1
        rnd = random.Random("%s-%d" % (rid, st["g"]))
        st["children"], st["kids"] = [], {}
        jobs = []
        for c in range(1, lam + 1):
            kid = {w: round(v * math.exp(sigma * rnd.gauss(0, 1)), 4) for w, v in st["parent"].items()}
            jid = "%s-g%02d-c%d" % (rid, st["g"], c)
            st["children"].append(jid)
            st["kids"][jid] = kid
            jobs.append(self.census_job(jid, "tune %s: generation %d child %d" % (rid, st["g"], c),
                                        "".join("%s = %s\n" % kv for kv in kid.items()), "", seeds, seed0, secs, mp))
        self.rsave(rid, st)
        return jobs

    # The learning harness (train, gauntlet, clips, school): one job.
    def harness_run(self, rid, kind, p, st):
        if st["status"] != "new":
            jid = st["job"]
            if jid in self.ids("done"):
                st["status"] = "done"
            elif jid in self.ids("failed", "stopped"):
                st["status"] = "failed"
            self.rsave(rid, st)
            return []
        jid = "%s-%s" % (rid, kind)
        env = {"train": "", "gauntlet": "GAUNTLET=1 ", "clips": "CLIPS=1 ", "school": "SCHOOL=1 "}[kind]
        send = []
        for k in ("start", "check"):
            if p.get(k):
                src = os.path.join(self.tc, p[k]) if not os.path.isabs(p[k]) else p[k]
                send.append({"src": src, "dst": "~/%s/trials/%s.%s.bin" % (self.home, jid, k)})
                env += "%s=$Q/trials/%s.%s.bin " % (k.upper(), jid, k)
        # The harness's own names, where the Makefile renamed them.
        alias = {"gauntlet": {"runs": "GAUNTLET_RUNS"}}.get(kind, {})
        for k, v in p.items():
            if k not in ("start", "check", "host"):
                env += "%s=%s " % (alias.get(k, k.upper()), shlex.quote(str(v)))
        cmd = (
            "set -e; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:$PATH\n"
            "Q=~/%s; C=%s; O=$Q/out/%s; mkdir -p $O $Q/trials\n"
            "cd $C/wt && %sOUT=$O THREADS=${THREADS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)} "
            "$C/census-bin --ignored learn::harness::train --nocapture 2>&1 | tee $O/harness.log | tail -20\n"
        ) % (self.home, self.cdir(), jid, env)
        job = {"id": jid, "kind": kind, "title": "%s %s" % (kind, rid), "needs": "host:%s" % p["host"] if p.get("host") else "cpu",
               "after_host": self.after_host(), "cmd": cmd, "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 72 * 3600}
        if send:
            job["send"] = send
        self.rsave(rid, {"status": "running", "job": jid, "commit": self.short()})
        return [job]

    # Recordings for behaviour cloning: a shard a job, streamed to the store.
    def record_run(self, rid, p, st, n):
        name = p.get("run", rid)
        seed0, count, per, secs = int(p.get("seed0", 9100000)), int(p.get("count", 120)), int(p.get("per", 12)), int(p.get("secs", 600))
        store = p.get("store", next((h["target"] for h in self.hosts if "gpu" in h.get("caps", [])), None))
        if not store:
            st.update({"status": "failed", "why": "no store: name one (store=user@host) or a gpu host"})
            self.rsave(rid, st)
            return []
        if st["status"] == "new":
            st = {"status": "running", "next": seed0, "end": seed0 + count, "jobs": [], "commit": self.short(), "store": store, "run": name}
        jobs = []
        while st["next"] < st["end"] and len(jobs) < max(1, n):
            s0, k = st["next"], min(per, st["end"] - st["next"])
            jid = "%s-%d" % (rid, s0)
            cmd = (
                "set -e -o pipefail; Q=~/%s; C=%s; mkdir -p $Q/out\n"
                "cd $C/wt && RECORD=$Q/out/%s.bin SEED0=%d SEEDS=%d SECS=%d THREADS=$(nproc 2>/dev/null || sysctl -n hw.ncpu) "
                "$C/census-bin --ignored learn::harness::train --nocapture 2>&1 | tail -4\n"
                "[ $(wc -c < $Q/out/%s.bin) -gt 16 ]\n"
            ) % (self.home, self.cdir(), jid, s0, k, secs, jid)
            jobs.append({"id": jid, "kind": "record", "title": "record %s: seeds %d to %d" % (name, s0, s0 + k - 1),
                         "needs": "cpu", "after_host": self.after_host(), "cmd": cmd, "timeout_s": 12 * 3600,
                         "deliver": {"src": "~/%s/out/%s.bin" % (self.home, jid), "to": store, "dst": "~/fleet/%s/shards/%d.bin" % (name, s0)}})
            st["jobs"].append(jid)
            st["next"] += k
        done, gone = self.ids("done"), self.ids("failed", "stopped")
        if st["next"] >= st["end"] and all(j in done or j in gone for j in st["jobs"]) and not jobs:
            st["status"] = "done"
            st["failed"] = [j for j in st["jobs"] if j in gone]
        self.rsave(rid, st)
        return jobs


    # ---- push permission: what a template must have for Approve to mean anything ----
    def push_target(self):
        p = self.cfg.get("push") or {}
        return p.get("remote", "origin"), p.get("branch", "main")

    def push_check(self):
        """Can this coordinator push the project's branch? A dry-run push of
        the remote's own head to it: it reaches the remote, proves the
        credentials, and changes nothing. Kept in plan/push.json."""
        remote, branch = self.push_target()
        url = run(["git", "-C", self.tc, "remote", "get-url", "--push", remote]).stdout.strip()
        run(["git", "-C", self.tc, "fetch", "-q", remote], timeout=120)
        head = run(["git", "-C", self.tc, "rev-parse", "%s/%s" % (remote, branch)]).stdout.strip()
        if not url or not head:
            ok, detail = False, "no remote %s with a branch %s in %s" % (remote, branch, self.tc)
        else:
            r = run(["git", "-C", self.tc, "push", "--dry-run", "--porcelain", remote, "%s:refs/heads/%s" % (head, branch)], timeout=120)
            ok = r.returncode == 0
            detail = "ok" if ok else (r.stderr.strip().splitlines() or ["refused"])[-1][:200]
        st = {"ok": ok, "remote": remote, "url": url, "branch": branch, "detail": detail, "checked": time.strftime("%Y-%m-%dT%H:%M:%S")}
        save(os.path.join(self.pd, "push.json"), st)
        if not ok:
            self.flag("push", "no push permission to %s %s: %s" % (url or remote, branch, detail), key="push:%s" % time.strftime("%Y-%m-%d"))
        return st

    # ---- promotion: a candidate prepared for treadcore, then accepted -----------
    # Paul, 28 October: "The process needs to be scripted without
    # intervention ... reword them to say Paul Accepts when auto promote is
    # on, and ssh credentials allow. Else they self modify, and have a flag
    # for please resolve this by commiting the weights and nets."
    #
    # A prepared promotion (its tests and families passed on a branch) is
    #   accepted:      auto mode on and this coordinator can push: Paul
    #                  Accepts, by his standing word; it is pushed to main and
    #                  tagged brain-RID;
    #   self-modified: otherwise: it goes onto the local line, a branch
    #                  orrery/accepted in this checkout (main and the
    #                  promotions not on it). The fleet builds the local line
    #                  (a git bundle sent with each build), so the work goes on
    #                  with the new values, and a flag asks Paul to resolve it:
    #                  commit the weights and nets to main. When main holds the
    #                  local line (Paul pushed it, or the credentials came back
    #                  and auto mode pushed it), the line ends.
    # Nothing waits for a person.
    LINE = "orrery/accepted"
    def evidence(self):
        """Every candidate with its weights and evidence: the census, the two
        held-out reads, and its families. {name: {...}}."""
        fam = load(os.path.join(self.pd, "families.json"), {})
        out = {}
        # This queue's: trials that passed both held-out reads.
        for jid, ds in self.held_pairs().items():
            if self.held_pass(ds):
                tw = glob.glob(os.path.join(self.q, "results", jid, self.home, "out", jid, "*", "trial.weights"))
                c = self.job_census(jid)
                held = [self.job_census(d.rstrip("/").split(os.sep)[-2]) for d in ds]
                if tw and c and all(held):
                    # Current: its census and both reads ran on today's
                    # defaults. Only a current candidate may promote itself.
                    commits = [self.census_commit(os.path.dirname(tw[0]))] + [self.census_commit(d) for d in ds]
                    out["%s/%s" % (self.name, jid)] = {"weights": open(tw[0]).read(), "census": c[:2], "held": [h[:2] for h in held],
                                                       "commits": commits, "current": all(self.same_defaults(x) for x in commits)}
        # Each converge run's: its progress lines.
        for run_dir in (self.cfg.get("families") or {}).get("converge", []):
            run_dir = os.path.join(self.tc, run_dir) if not os.path.isabs(run_dir) else run_dir
            stamp = os.path.basename(run_dir.rstrip("/"))
            try:
                lines = open(os.path.join(run_dir, "progress.txt")).read().splitlines()
            except OSError:
                continue
            census, held = {}, []
            for l in lines:
                m = re.match(r"^\S+ (gen\d+/\S+) every map\s+([-+][\d.]+) \+/- ([\d.]+)", l)
                if m:
                    census[m.group(1)] = (float(m.group(2)), float(m.group(3)))
                    held = []
                    continue
                m = re.match(r"^\S+\s+held out from \d+: every map\s+([-+][\d.]+) \+/- ([\d.]+)", l)
                if m:
                    held.append((float(m.group(1)), float(m.group(2))))
                    continue
                m = re.match(r"^\S+\s+(gen\d+/\S+): pass pass, CANDIDATE", l)
                if m and os.path.exists(os.path.join(run_dir, m.group(1) + ".weights")):
                    out["%s/%s" % (stamp, m.group(1))] = {"weights": open(os.path.join(run_dir, m.group(1) + ".weights")).read(),
                                                         "census": census.get(m.group(1)), "held": held[-2:]}
        for k, v in out.items():
            f = fam.get(k)
            v["families"] = f["verdict"] if f else "not read"
            v["held_mean"] = sum(h[0] for h in v["held"]) / len(v["held"]) if v["held"] else None
        return out

    def strongest(self):
        ev = {k: v for k, v in self.evidence().items() if v["families"] == "holds" and v["held_mean"] is not None}
        return max(ev.items(), key=lambda kv: kv[1]["held_mean"]) if ev else (None, None)

    def promote_run(self, rid, p, st):
        if st["status"] in ("prepared", "approved", "self-modified", "rejected"):
            return []
        if st["status"] == "new":
            # Without push permission the promotion still runs: it is
            # prepared here and self-modifies (the local line).
            can_push = self.push_check()["ok"]
            name = p.get("candidate", "@strongest")
            if name == "@strongest":
                name, ev = self.strongest()
            else:
                ev = self.evidence().get(name)
            if p.get("weights"):
                name, ev = p.get("name", "weights"), {"weights": self.text(p["weights"]), "census": None, "held": [], "families": "not read"}
            if not ev:
                self.rsave(rid, {"status": "failed", "why": "no candidate %s with evidence (families held, two held-out reads)" % name})
                return []
            if ev.get("families") == "UNDER":
                self.rsave(rid, {"status": "failed", "why": "%s falls under a family floor" % name})
                return []
            # What would change, read against the coordinator's checkout of main.
            tmp = os.path.join(self.rdir(rid), "promote.weights")
            open(tmp, "w").write(ev["weights"])
            r = run([sys.executable, "-I", os.path.join(self.tc, "scripts", "set-defaults.py"), tmp, "--check"], cwd=self.tc)
            if r.returncode != 0:
                self.rsave(rid, {"status": "failed", "why": r.stderr.strip()[-200:]})
                return []
            changed = r.stdout.strip().splitlines()
            fmt = lambda c: "%+.2f +/- %.2f" % tuple(c) if c else "?"
            msg = ("promote %s as the brain's defaults -- %s; census %s, held out %s; families %s; prepared by orrery (queue %s, run %s), "
                   "%s\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\n") % (
                name, "; ".join(changed) or "no change", fmt(ev.get("census")), " and ".join(fmt(h) for h in ev.get("held", [])) or "?",
                ev.get("families"), self.name, rid,
                "Paul Accepts: orrery's auto mode, with push permission (Paul, 28 October)" if self.auto() and can_push
                else "self-modified: on orrery's local line %s until it is committed to main" % self.LINE)
            jid = "%s-promote" % rid
            branch = "promote/%s-%s" % (self.name, rid)
            b1 = base64.b64encode(ev["weights"].encode()).decode()
            b2 = base64.b64encode(msg.encode()).decode()
            # Without push permission the commit must be made where the
            # local line is: here.
            host = p.get("host", "mac") if can_push else "mac"
            # It is tested on what the fleet runs: the local line, if there is one.
            base = self.LINE if self.state.get("local_line") else "origin/main"
            out = "~/%s/out/%s" % (self.home, jid)
            cmd = (
                "set -e -o pipefail; [ -f ~/.cargo/env ] && . ~/.cargo/env; export PATH=$HOME/.cargo/bin:/opt/homebrew/bin:$PATH\n"
                "T=~/code/treadcore; W=~/%s/promote/%s; B=%s; O=%s; mkdir -p $O\n"
                "cd $T && git fetch -q origin\n"
                "git worktree remove --force $W 2>/dev/null || true; git worktree prune; git branch -D $B 2>/dev/null || true\n"
                "git worktree add -q -b $B $W %s\n"
                "cd $W && echo %s | base64 -d > $O/promote.weights && python3 -I scripts/set-defaults.py $O/promote.weights | tee $O/changed.txt\n"
                # Its own target folder: two worktrees in one share the test
                # binary's file name, and cargo can call another's fresh (LR 117).
                "export CARGO_TARGET_DIR=$W/target\n"
                "cargo test -q -p treadcore-brain --release > $O/tests.txt 2>&1 || { tail -30 $O/tests.txt; echo 'the tests failed: not committed'; exit 1; }\n"
                "grep 'test result' $O/tests.txt\n"
                "cargo test -q -p treadcore-brain --release -- --ignored every_family_holds_its_floor --nocapture > $O/families-full.txt 2>&1 || { grep -E '^family|panicked|under' $O/families-full.txt; echo 'a family fell under its floor: not committed'; exit 1; }\n"
                "grep -E '^family' $O/families-full.txt > $O/families.txt; cat $O/families.txt\n"
                "git add source/treadcore-brain/src/weights.rs && echo %s | base64 -d > $O/msg.txt && git commit -q -F $O/msg.txt\n"
                "%s\n"
                "git rev-parse HEAD > $O/commit.txt; echo \"prepared $(cat $O/commit.txt) on $B\"\n"
            ) % (self.home, rid, branch, out, base, b1, b2,
                 "git push -q %s $B" % self.push_target()[0] if can_push else "echo 'the branch is kept here: no push permission'")
            st = {"status": "running", "job": jid, "candidate": name, "branch": branch, "changed": changed, "can_push": can_push,
                  "evidence": {k: ev.get(k) for k in ("census", "held", "families")}}
            self.rsave(rid, st)
            return [{"id": jid, "kind": "promote", "title": "promote %s: tests and families on a branch" % name, "needs": "host:" + host,
                     "cmd": cmd, "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 4 * 3600}]
        # Running: the branch is prepared when the job is done.
        if st["job"] in self.ids("done"):
            c = glob.glob(os.path.join(self.q, "results", st["job"], self.home, "out", st["job"], "commit.txt"))
            st["commit"] = open(c[0]).read().strip() if c else ""
            st["status"] = "prepared"
            st["detail"] = "%s: prepared (%s)" % (st["branch"], st["commit"][:7])
            self.rsave(rid, st)
            accept = self.auto() and self.push_check()["ok"]
            try:
                if accept:
                    self._approve(rid)
                else:
                    self.self_modify(rid)
                st = self.rstate(rid)
            except SystemExit as e:
                # main moved under it, or the replay failed: the local line
                # takes it instead, so the values are not lost.
                why = str(e).splitlines()[0]
                try:
                    self.self_modify(rid, because="Paul Accepts failed (%s)" % why)
                    st = self.rstate(rid)
                except SystemExit as e2:
                    st["status"] = "failed"
                    st["why"] = "accepting: %s; self-modifying: %s" % (why, str(e2).splitlines()[0])
            self.tick({"kind": "promote", "run": rid, "host": "mac", "improved": True, "d": None, "se": None,
                       "vars": changed_vars(st["changed"])})
        elif st["job"] in self.ids("failed", "stopped"):
            st["status"] = "failed"
            st["why"] = self.promote_failure(st["job"])
        self.rsave(rid, st)
        return []

    def promote_failure(self, jid):
        """Which step of a promotion failed, read from its log: the brain's
        tests (and the first that failed), a family under its floor, or the
        job itself."""
        try:
            log = open(os.path.join(self.q, "logs", jid + ".log")).read()
        except OSError:
            log = ""
        if "the tests failed" in log:
            m = re.search(r"^(\S+) --- FAILED", log, re.M)
            return "the brain's tests failed%s: see logs/%s.log" % (" (%s)" % m.group(1) if m else "", jid)
        if "a family" in log and "under" in log:
            m = re.search(r"^family (\S+).*under", log, re.M)
            return "a family fell under its floor%s: see logs/%s.log" % (" (%s)" % m.group(1) if m else "", jid)
        return "the promotion job failed: see logs/%s.log" % jid

    def approve(self, rid):
        """Paul's word, by hand: a prepared or self-modified promotion is
        pushed to main, so the fleet trains on it at its next top-up."""
        st = self._approve(rid)
        self.keep()
        print("%s approved: main is %s; the fleet builds it at its next top-up" % (rid, st["commit"][:7]))

    def _approve(self, rid):
        st = self.rstate(rid)
        if st.get("status") == "self-modified":
            # By hand: the local line goes to main, this one with it.
            self.resolve()
            return self.rstate(rid)
        if st.get("status") != "prepared" or not st.get("commit"):
            raise SystemExit("%s is not a prepared promotion (%s)" % (rid, st.get("status")))
        remote, branch = self.push_target()
        r = run(["git", "-C", self.tc, "push", remote, "%s:refs/heads/%s" % (st["commit"], branch)])
        if r.returncode != 0:
            # main moved while it was tested. If nothing under source/ moved
            # (records, reports, docs), what was tested is what main would
            # build: the promotion commit is replayed on main and pushed.
            # Otherwise it must be tested again.
            run(["git", "-C", self.tc, "fetch", "-q", remote])
            tip = "%s/%s" % (remote, branch)
            base = run(["git", "-C", self.tc, "merge-base", st["commit"], tip]).stdout.strip()
            moved = run(["git", "-C", self.tc, "diff", "--quiet", base, tip, "--", "source/"]).returncode != 0
            if not base or moved:
                raise SystemExit("main's brain source has moved since it was prepared: ask for the promotion again\n" + r.stderr[-300:])
            w = os.path.join(self.rdir(rid), "replay")
            run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
            run(["git", "-C", self.tc, "worktree", "prune"])
            r = run(["git", "-C", self.tc, "worktree", "add", "-q", "--detach", w, tip])
            r = r if r.returncode else run(["git", "-C", w, "cherry-pick", "--allow-empty", st["commit"]])
            new = run(["git", "-C", w, "rev-parse", "HEAD"]).stdout.strip() if r.returncode == 0 else ""
            r = r if r.returncode else run(["git", "-C", self.tc, "push", remote, "%s:refs/heads/%s" % (new, branch)])
            run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
            if r.returncode != 0:
                raise SystemExit("main moved, and replaying the promotion on it failed\n" + r.stderr[-300:])
            st["tested"] = st["commit"]
            st["commit"] = new
        run(["git", "-C", self.tc, "fetch", "-q", remote])
        self.push_check()
        st["status"] = "approved"
        st["accepted"] = "Paul Accepts (auto mode)" if self.auto() else "Paul approved"
        st["detail"] = "%s: on main at %s" % (st["accepted"], st["commit"][:7])
        st["approved"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        # A micro version (Paul, 28 October: "essentially creating micro
        # versions of this git submission. its part of the program"): an
        # annotated tag brain-RID on the promotion's commit, pushed with it.
        # Never v*: a v tag starts the packages workflow, a release only Paul
        # asks for.
        tag = self.micro_tag(rid, st)
        if tag:
            st["tag"] = tag
            st["detail"] += ", tagged %s" % tag
        self.rsave(rid, st)
        # Every candidate's families again, on the new defaults; and the
        # commit is read anew, so this pass's prep jobs build it everywhere.
        self.state["families_queued"] = []
        self._sha = None
        self.flag("promote", "promotion %s: %s: %s is main at %s; every machine builds it next" % (
            rid, st["accepted"], st.get("candidate"), st["commit"][:7]), key="approved:%s" % rid, progress=True)
        return st

    def self_modify(self, rid, because=None):
        """A prepared promotion onto the local line (LINE): main and the
        promotions not on it. The fleet builds it; a flag asks Paul to
        commit the weights and nets."""
        st = self.rstate(rid)
        remote, branch = self.push_target()
        run(["git", "-C", self.tc, "fetch", "-q", remote], timeout=120)
        tip = "%s/%s" % (remote, branch)
        line = self.state.get("local_line")
        base = self.LINE if line else tip
        w = os.path.join(self.rdir(rid), "line")
        run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
        run(["git", "-C", self.tc, "worktree", "prune"])
        r = run(["git", "-C", self.tc, "worktree", "add", "-q", "--detach", w, base])
        # The prepared commit was made on what the fleet ran then; on the
        # line as it is now, it is replayed (cherry-picked) when needed.
        same = run(["git", "-C", self.tc, "merge-base", "--is-ancestor", base, st["commit"]]).returncode == 0
        r = r if r.returncode else run(["git", "-C", w, "merge", "-q", "--ff-only", st["commit"]] if same
                                        else ["git", "-C", w, "cherry-pick", "--allow-empty", st["commit"]])
        new = run(["git", "-C", w, "rev-parse", "HEAD"]).stdout.strip() if r.returncode == 0 else ""
        run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
        if not new:
            raise SystemExit("the promotion does not apply on the local line: " + (r.stderr or r.stdout)[-300:])
        run(["git", "-C", self.tc, "branch", "-f", self.LINE, new])
        line = line or {"promotions": []}
        line["promotions"].append(rid)
        line["commit"] = new
        line["since"] = line.get("since") or time.strftime("%Y-%m-%dT%H:%M:%S")
        self.state["local_line"] = line
        self.bundle()
        st["status"] = "self-modified"
        st["tested"] = st["commit"]
        st["commit"] = new
        st["approved"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        why = because or ("auto mode is off" if not self.auto() else "no push permission (%s)" % self.push_check()["detail"])
        st["detail"] = "self-modified onto %s at %s (%s): the fleet builds it; main does not have it yet" % (self.LINE, new[:7], why)
        self.rsave(rid, st)
        self.state["families_queued"] = []
        self._sha = None
        self.resolve_flag()
        return st

    def local_line(self, main_sha, by="committed to main by Paul"):
        """The local line's head while main lacks it, else None. When main
        holds every promotion on it, the line ends; when main moved on
        without it, the line is replayed on main."""
        line = self.state.get("local_line")
        if not line:
            return None
        head = line.get("commit", "")
        if run(["git", "-C", self.tc, "merge-base", "--is-ancestor", head, main_sha]).returncode == 0:
            # Resolved: Paul committed the weights (or pushed the line).
            for rid in line["promotions"]:
                st = self.rstate(rid)
                if st.get("status") == "self-modified":
                    st["status"] = "approved"
                    st["accepted"] = by
                    st["self_modified_at"] = st.get("commit", "")
                    st["commit"] = head
                    st["detail"] = "%s: on main at %s (self-modified at %s before)" % (by, head[:7], st["self_modified_at"][:7])
                    self.rsave(rid, st)
            self.state.pop("local_line", None)
            self.flag("resolve", "the local line is on main (%s): resolved" % main_sha[:7], key="resolved:%s" % head[:12], progress=True)
            return None
        if run(["git", "-C", self.tc, "merge-base", "--is-ancestor", main_sha, head]).returncode == 0:
            return head
        # main moved on: the line's own commits again, on top of it.
        w = os.path.join(self.pd, "line-replay")
        run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
        run(["git", "-C", self.tc, "worktree", "prune"])
        own = run(["git", "-C", self.tc, "rev-list", "--reverse", "%s..%s" % (main_sha, head)]).stdout.split()
        r = run(["git", "-C", self.tc, "worktree", "add", "-q", "--detach", w, main_sha])
        for c in own:
            if r.returncode:
                break
            r = run(["git", "-C", w, "cherry-pick", "--allow-empty", c])
        new = run(["git", "-C", w, "rev-parse", "HEAD"]).stdout.strip() if r.returncode == 0 else ""
        if r.returncode:
            run(["git", "-C", w, "cherry-pick", "--abort"])
        run(["git", "-C", self.tc, "worktree", "remove", "--force", w])
        if not new:
            self.flag("resolve", "main moved and the local line %s does not replay on it: please resolve this by committing the weights and nets by hand"
                      % head[:7], key="replay:%s:%s" % (head[:12], main_sha[:12]))
            return head
        run(["git", "-C", self.tc, "branch", "-f", self.LINE, new])
        line["commit"] = new
        self.state["local_line"] = line
        self.bundle()
        return new

    def bundle(self):
        """The local line's own commits as a git bundle (plan/accepted.bundle),
        sent with each build so every machine has them without GitHub."""
        remote, branch = self.push_target()
        f = os.path.join(self.pd, "accepted.bundle")
        r = run(["git", "-C", self.tc, "bundle", "create", f + ".part", self.LINE, "^%s/%s" % (remote, branch)])
        if r.returncode == 0:
            os.replace(f + ".part", f)

    def pending_nets(self):
        """Nets not committed: files in the checkout's nets/ git does not
        track, and the net rounds' best when it is a new one."""
        out = []
        r = run(["git", "-C", self.tc, "ls-files", "--others", "--exclude-standard", "nets"])
        out += [l.strip() for l in r.stdout.splitlines() if l.strip().endswith(".bin")]
        best = load(os.path.join(self.pd, "best.json"), {}) or {}
        if best.get("round"):
            out.append("plan/best.bin (%s)" % best.get("name"))
        return out

    def resolve_flag(self):
        """Paul, 28 October: "have a flag for please resovle this by
        commiting the weights and nets". Raised while the local line lacks
        main, naming what to commit."""
        line = self.state.get("local_line")
        if not line:
            return
        nets = self.pending_nets()
        remote, branch = self.push_target()
        self.flag("resolve", "please resolve this by committing the weights and nets: %d promotion(s) (%s) are on %s at %s, not on %s/%s%s. "
                  "From a machine that can push: git push %s %s:%s, or fleet-queue.py %s resolve" % (
                      len(line["promotions"]), ", ".join(line["promotions"]), self.LINE, line["commit"][:7], remote, branch,
                      "; nets not committed: " + ", ".join(nets) if nets else "", remote, self.LINE, branch, self.q),
                  key="resolve:%s" % line["commit"][:12])

    def resolve(self):
        """The local line to main, when this coordinator can push."""
        line = self.state.get("local_line")
        if not line:
            print("no local line: main has every promotion")
            return
        ps = self.push_check()
        if not ps["ok"]:
            raise SystemExit("no push permission (%s): push %s from a machine that can" % (ps["detail"], self.LINE))
        remote, branch = self.push_target()
        main = run(["git", "-C", self.tc, "rev-parse", "%s/%s" % (remote, branch)]).stdout.strip()
        head = self.local_line(main)
        if not head:
            return
        r = run(["git", "-C", self.tc, "push", remote, "%s:refs/heads/%s" % (head, branch)])
        if r.returncode != 0:
            raise SystemExit("the push failed: " + r.stderr[-300:])
        run(["git", "-C", self.tc, "fetch", "-q", remote])
        self.local_line(head, by="Paul Accepts (auto mode)" if self.auto() else "Paul approved")
        for rid in line["promotions"]:
            st = self.rstate(rid)
            tag = self.micro_tag(rid, st)
            if tag:
                st["tag"] = tag
                st["detail"] += ", tagged %s" % tag
                self.rsave(rid, st)
        self._sha = None
        print("the local line is main at %s" % head[:7])

    def micro_tag(self, rid, st):
        tag = "brain-%s" % rid
        remote, _ = self.push_target()
        changed = [c.strip() for c in st.get("changed", []) if not c.strip().startswith("Tactics::default()")]
        ev = st.get("evidence", {})
        held = " and ".join("%+.2f" % h[0] for h in ev.get("held", []))
        msg = "%s: %s as the brain's defaults\n\n%s\n\nheld out %s; families %s; queue %s, run %s, %s" % (
            tag, st.get("candidate"), "\n".join(changed), held, ev.get("families", "?"), self.name, rid, time.strftime("%Y-%m-%dT%H:%M:%S"))
        if run(["git", "-C", self.tc, "rev-parse", "-q", "--verify", "refs/tags/" + tag]).returncode != 0:
            if run(["git", "-C", self.tc, "tag", "-a", tag, "-m", msg, st["commit"]]).returncode != 0:
                return ""
        r = run(["git", "-C", self.tc, "push", "-q", remote, "refs/tags/%s" % tag])
        return tag if r.returncode == 0 else ""

    def runs_status(self):
        out = []
        for r in self.runs():
            st = self.rstate(r["id"])
            what = {"census": lambda: "%d censuses" % len(st.get("items", [])),
                    "spsa": lambda: "step %s" % st.get("k"), "tune": lambda: "generation %s, fitness %s" % (st.get("g"), st.get("fit")),
                    "record": lambda: "next seed %s of %s" % (st.get("next"), st.get("end"))}
            k = r["kind"] if r["kind"] in what else "census" if r["kind"] in self.CENSUS_KINDS else None
            detail = st.get("detail") or (what[k]() if k and st.get("status") != "new" else "")
            if r["kind"] == "promote" and st.get("candidate") and st.get("status") == "running":
                detail = "%s: tests and families on %s" % (st["candidate"], st.get("branch"))
            out.append({"id": r["id"], "kind": r["kind"], "status": st.get("status"), "why": st.get("why", ""),
                        "detail": detail, "params": r.get("params", {}), "by": r.get("by", ""),
                        "candidate": st.get("candidate", ""), "hosts": st.get("hosts", {})})
        return out

    # ---- the ticker: every finished census, each coefficient it moved ----------------
    def tick(self, e):
        e.setdefault("t", time.strftime("%Y-%m-%dT%H:%M:%S"))
        with open(os.path.join(self.pd, "ticker.jsonl"), "a") as f:
            f.write(json.dumps(e) + "\n")

    def ticker_scan(self):
        """A ticker line for each weights census not yet ticked: each variable
        it moved, the value, the offset from the default, the difference and
        which machine read it; "improved" past two errors with no map below."""
        seen = set(self.state.setdefault("ticked", []))
        dflt = parse_weights(open(self.defaults()).read()) if self.defaults() else {}
        for d in self.census_dirs():
            jid = d.rstrip("/").split(os.sep)[-2]
            if jid in seen or not jid[:1] in ("w", "h"):
                continue
            c = self.job_census(jid)
            if not c:
                continue
            try:
                vars_ = parse_weights(open(os.path.join(d, "trial.weights")).read())
            except OSError:
                vars_ = {}
            self.tick({"kind": "weights", "job": jid, "d": c[0], "se": c[1], "host": c[3], "improved": bool(self.passes(d)),
                       "vars": {k: {"value": v, "offset": round(v - dflt[k], 4) if k in dflt else None} for k, v in vars_.items()}})
            self.state["ticked"].append(jid)

    # ---- the keeper's calls ---------------------------------------------------------------
    def candidate_states(self):
        """plan/candidates.json: each of this queue's candidates, current (its
        census and both reads on today's defaults, so it may promote itself)
        or on older defaults, with the commit it was read at. The Candidates
        page marks each line with it."""
        out = {}
        for k, v in self.evidence().items():
            if k.startswith(self.name + "/"):
                out[k.split("/", 1)[1]] = {"current": bool(v.get("current")), "commit": (v.get("commits") or [None])[0]}
        save(os.path.join(self.pd, "candidates.json"), out)

    def raise_found(self):
        """Flags for what the fleet found: a trial past both held-out reads,
        and a candidate under a family's floor."""
        for jid, ds in self.held_pairs().items():
            if self.held_pass(ds):
                c = self.job_census(jid)
                held = [self.job_census(d.rstrip("/").split(os.sep)[-2]) for d in ds]
                tw = glob.glob(os.path.join(self.q, "results", jid, self.home, "out", jid, "*", "trial.weights"))
                what = short_moved(open(tw[0]).read()) if tw else jid
                self.flag("candidate", "%s passed both held-out reads (%s): %s" % (jid, ", ".join("%+.2f" % h[0] for h in held if h), what), key="candidate:%s" % jid, progress=True)
        fam = load(os.path.join(self.pd, "families.json"), {}) or {}
        for name, v in fam.items():
            if v.get("verdict") == "UNDER":
                self.flag("families", "%s falls under a family's floor" % name, key="under:%s" % name)

    # ---- crucible: the second job, on the GPU lanes ----------------------------------------
    # Paul, 28 October: "i want to see a button to add more gpu CUDA to the
    # mix, add crucible." Each GPU machine given a crucible lane (machines.py
    # cuda-lanes) is an island: its own population, trained a segment at a
    # time (plan.json "crucible": gens, pop, eps), its state kept on the
    # machine between segments. Each segment sends back its champion and its
    # generations, and takes the other islands' newest champions into its
    # hall of fame. The source is sent with every segment and built when it
    # changed, so the islands run what this Mac has.
    #
    # The ladder (Paul, 28 October: "islands' own past champions ...
    # battling known milestones in the past is a good idea"; "when they find
    # good values at the same time, how do we keep consistent generations
    # with keeping data"). Every segment's champion is kept
    # (crucible/champions/, a line in crucible/lineage.tsv). A champion
    # becomes the fleet's next milestone, M(n+1), only when its exam was
    # against M(n), the newest one, and it beat M(n) by more than two
    # standard errors (wins less losses), without losing to the ladder on
    # balance. When two islands qualify in the same pass, the larger margin
    # (in standard errors) becomes M(n+1); the other is kept, and meets
    # M(n+1) in its next segment. So the milestones are one line of
    # generations, each beating the one before, whichever card found it, and
    # nothing a card found is lost. The ladder and the imports are slot
    # files the keeper reads when it dispatches a segment, so a segment
    # queued six ahead still meets the newest milestones.
    LADDER_SLOTS = 8

    def crucible_dir(self, *a):
        return os.path.join(self.q, "crucible", *a)

    def ladder_rungs(self, ms):
        """At most LADDER_SLOTS milestones, oldest first: the newest half,
        and the older ones spread evenly."""
        n = self.LADDER_SLOTS
        if len(ms) <= n:
            return list(ms)
        new = ms[-(n // 2):]
        old = ms[:-(n // 2)]
        k = n - len(new)
        return [old[round(i * (len(old) - 1) / max(1, k - 1))] for i in range(k)] + new

    def ladder_write(self):
        """The slot files and ladder.txt (slot, milestone) the segments are sent."""
        lad = self.state.setdefault("crucible_ladder", {"milestones": []})
        d = self.crucible_dir("ladder")
        os.makedirs(d, exist_ok=True)
        rungs = self.ladder_rungs(lad["milestones"])
        lines = []
        for i in range(self.LADDER_SLOTS):
            dst = os.path.join(d, "slot-%d.bin" % i)
            if i < len(rungs):
                shutil.copyfile(self.crucible_dir("milestones", rungs[i]["id"] + ".bin"), dst + ".part")
                lines.append("slot-%d.bin\t%s" % (i, rungs[i]["id"]))
            else:
                open(dst + ".part", "wb").close()
            os.replace(dst + ".part", dst)
        with open(os.path.join(d, "ladder.txt.part"), "w") as f:
            f.write("".join(l + "\n" for l in lines))
        os.replace(os.path.join(d, "ladder.txt.part"), os.path.join(d, "ladder.txt"))

    def crucible_exam(self, name, jid):
        """A finished segment's champion kept, its exam read: a row for the lineage."""
        out = os.path.join(self.q, "results", jid, self.home, "out", jid)
        champ = os.path.join(out, "champ.bin")
        if not os.path.exists(champ):
            return None
        os.makedirs(self.crucible_dir("champions"), exist_ok=True)
        os.makedirs(self.crucible_dir("latest"), exist_ok=True)
        shutil.copyfile(champ, self.crucible_dir("champions", jid + ".bin"))
        shutil.copyfile(champ, self.crucible_dir("latest", name + ".bin.part"))
        os.replace(self.crucible_dir("latest", name + ".bin.part"), self.crucible_dir("latest", name + ".bin"))
        gen, last_yard = 0, None
        try:
            rows = open(os.path.join(out, "gens.tsv")).read().splitlines()[1:]
            gen = int(rows[-1].split("\t")[0]) + 1 if rows else 0
            ys = [float(r.split("\t")[7]) for r in rows if len(r.split("\t")) > 9 and float(r.split("\t")[7]) >= 0]
            last_yard = ys[-1] if ys else None
        except (OSError, ValueError):
            pass
        exam = []
        try:
            for line in open(os.path.join(out, "exam.tsv")).read().splitlines()[1:]:
                o, w, d, l, n = line.split("\t")
                exam.append((o, float(w), float(d), float(l), int(n)))
        except (OSError, ValueError):
            pass
        row = {"job": jid, "island": name, "gen": gen, "yard": None, "against": None, "w": None, "d": None, "l": None,
               "margin_se": None, "ladder_mean": None, "result": "kept"}
        rungs = [e for e in exam if e[0] != "yardstick"]
        # A segment from before the exam: its last yardstick read instead.
        row["yard"] = last_yard
        for e in exam:
            if e[0] == "yardstick":
                row["yard"] = e[1]
        if rungs:
            o, w, d, l, n = rungs[-1]
            se = math.sqrt(max(1e-9, (w + l - (w - l) ** 2) / n))
            row.update({"against": o, "w": w, "d": d, "l": l, "margin_se": (w - l) / se,
                        "ladder_mean": sum(r[1] - r[3] for r in rungs) / len(rungs)})
        return row

    def crucible_ladder(self, fresh):
        """Finished segments, all islands at once: each kept, and the
        milestone rule applied in order of margin."""
        lad = self.state.setdefault("crucible_ladder", {"milestones": []})
        rows = [r for r in (self.crucible_exam(n, j) for n, j in fresh) if r]
        top = lad["milestones"][-1]["id"] if lad["milestones"] else None
        # Strongest first: the margin over the newest milestone; with no
        # milestone yet, the yardstick's wins.
        rows.sort(key=lambda r: (-(r["margin_se"] if r["margin_se"] is not None else -1e9), -(r["yard"] or 0), r["island"]))
        os.makedirs(self.crucible_dir("milestones"), exist_ok=True)
        for r in rows:
            if top is None:
                ok = r is rows[0]
                why = "the first milestone (yardstick %.3f)" % (r["yard"] or 0)
            elif r["against"] != top:
                ok, why = False, "%s, not the newest %s: meets it next segment" % (
                    "examined against %s" % r["against"] if r["against"] else "no exam against the ladder", top)
            else:
                ok = r["margin_se"] > 2 and r["ladder_mean"] >= 0
                why = "beat %s by %.1f se, ladder %+.3f" % (top, r["margin_se"], r["ladder_mean"])
            if ok:
                mid = "M%04d-%s-g%d" % (len(lad["milestones"]) + 1, r["island"], r["gen"])
                shutil.copyfile(self.crucible_dir("champions", r["job"] + ".bin"), self.crucible_dir("milestones", mid + ".bin"))
                lad["milestones"].append({"id": mid, "job": r["job"], "island": r["island"], "gen": r["gen"],
                                          "after": top, "margin_se": r["margin_se"], "made": time.strftime("%Y-%m-%dT%H:%M:%S")})
                r["result"] = "milestone %s: %s" % (mid, why)
                top = mid
                self.flag("crucible", "crucible: %s is milestone %s (%s)" % (r["job"], mid, why), key="milestone:%s" % mid, progress=True)
            else:
                r["result"] = "kept: %s" % why
            new = not os.path.exists(self.crucible_dir("lineage.tsv"))
            with open(self.crucible_dir("lineage.tsv"), "a") as f:
                if new:
                    f.write("time\tjob\tisland\tgen\tyard_win\tagainst\twin\tdraw\tloss\tmargin_se\tladder_mean\tresult\n")
                fm = lambda x: "-" if x is None else ("%.4f" % x if isinstance(x, float) else str(x))
                f.write("\t".join([time.strftime("%Y-%m-%dT%H:%M:%S"), r["job"], r["island"], str(r["gen"])] +
                                  [fm(r[k]) for k in ("yard", "against", "w", "d", "l", "margin_se", "ladder_mean")] + [r["result"]]) + "\n")
        self.ladder_write()
    def crucible_src(self):
        return os.path.expanduser(self.cfg.get("crucible", {}).get("source", "~/code/crucible"))

    def crucible_jobs(self):
        src = self.crucible_src()
        if not self.cuda or not os.path.exists(os.path.join(src, "src", "crucible.cu")):
            return []
        opt = dict({"gens": 50, "pop": 2048, "eps": 128, "yard": 10, "ahead": 6, "lad": 0.2, "draw": -1.0}, **self.cfg.get("crucible", {}))
        if not os.path.exists(self.crucible_dir("ladder", "ladder.txt")):
            self.ladder_write()
        cr = self.state.setdefault("crucible", {})
        done, bad = self.ids("done"), self.ids("failed", "stopped")
        # Each island keeps `ahead` segments queued, each after the one
        # before (its state file is the last one's), so the GPU lane is not
        # left idle between the keeper's plans.
        fresh = []
        for h in self.cuda:
            isl = cr.setdefault(h["name"], {"seg": 0, "last": None, "failures": 0, "gens": 0})
            isl.setdefault("queued", [isl["job"]] if isl.get("job") else [])
            isl.setdefault("next", isl["seg"] + len(isl["queued"]))
            isl.pop("job", None)
            live = []
            for j in isl["queued"]:
                if j in done:
                    isl.update({"seg": isl["seg"] + 1, "last": j, "failures": 0, "gens": isl["gens"] + opt["gens"]})
                    fresh.append((h["name"], j))
                elif j in bad:
                    isl["failures"] += 1
                else:
                    live.append(j)
            isl["queued"] = live
            if isl["failures"] >= 3:
                self.flag("crucible", "crucible on %s failed three segments in a row: see its logs" % h["name"], key="crucible:%s:%d" % (h["name"], isl["seg"]))
        self.crucible_ladder(fresh)
        jobs = []
        for h in self.cuda:
            name, isl = h["name"], cr[h["name"]]
            if isl["failures"] >= 3:
                continue
            while len(isl["queued"]) < opt["ahead"]:
                jid = "x-%s-%04d" % (name, isl["next"])
                isl["next"] += 1
                home = "%s/crucible" % self.home
                send = [{"src": os.path.join(src, f), "dst": "~/%s/%s" % (home, f)} for f in ("src/crucible.cu", "Makefile")]
                # Read when the keeper dispatches it: the other islands'
                # newest champions, and the ladder as it is then.
                imports = []
                for o in cr:
                    champ = self.crucible_dir("latest", o + ".bin")
                    if o != name and os.path.exists(champ):
                        send.append({"src": champ, "dst": "~/%s/in/%s.bin" % (home, o)})
                        imports.append("$H/in/%s.bin" % o)
                send += [{"src": self.crucible_dir("ladder", f), "dst": "~/%s/ladder/%s" % (home, f)}
                         for f in ["slot-%d.bin" % i for i in range(self.LADDER_SLOTS)] + ["ladder.txt"]]
                seed = 1000 + 97 * (sum(map(ord, name)) % 1000)
                cmd = (
                    "set -e; H=~/%s; O=~/%s/out/%s; mkdir -p $O $H/in\n"
                    "cd $H && make -s NVCC=${NVCC:-/usr/local/cuda/bin/nvcc} >/dev/null 2>&1 || make NVCC=${NVCC:-/usr/local/cuda/bin/nvcc}\n"
                    "L=''; [ -f $H/island.state ] && L=\"--load $H/island.state\"\n"
                    "I='%s'\n"
                    # The milestones by name, oldest first, from their slots.
                    "rm -rf $H/rungs; mkdir -p $H/rungs; R=''\n"
                    "while IFS=\"$(printf '\\t')\" read -r s m; do if [ -n \"$m\" ]; then cp $H/ladder/$s $H/rungs/$m.bin; R=\"$R${R:+,}$H/rungs/$m.bin\"; fi; done < $H/ladder/ladder.txt\n"
                    "./crucible --pop %d --eps %d --gens %d --yard %d --seed %d --lad %s --draw %s $L ${I:+--import $I} ${R:+--ladder $R} "
                    "--save $H/island.state --export $O/champ.bin --out $O/gens.tsv --exam $O/exam.tsv 2>&1 | tee $O/console.txt | grep '^#'\n"
                ) % (home, self.home, jid, ",".join(imports), opt["pop"], opt["eps"], opt["gens"], opt["yard"], seed, opt["lad"], opt["draw"])
                job = {"id": jid, "kind": "crucible", "title": "crucible island %s, segment %d" % (name, isl["next"] - 1),
                       "needs": "host:" + name, "cmd": cmd, "send": send, "fetch": "%s/out/%s" % (self.home, jid), "timeout_s": 4 * 3600}
                if isl["queued"]:
                    job["after"] = [isl["queued"][-1]]
                jobs.append(job)
                isl["queued"].append(jid)
        self.crucible_view()
        return jobs

    def crucible_view(self):
        """plan/crucible.json for orrery's Projects view: each island's
        segments, generations, last yardstick and speed, from its results."""
        out = {}
        for name, isl in self.state.get("crucible", {}).items():
            rows = []
            for d in sorted(glob.glob(os.path.join(self.q, "results", "x-%s-*" % name))):
                jid = os.path.basename(d)
                f = os.path.join(d, self.home, "out", jid, "gens.tsv")
                try:
                    for line in open(f).read().splitlines()[1:]:
                        rows.append(line.split("\t"))
                except OSError:
                    pass
            yard = [(int(r[0]) + 1, float(r[7])) for r in rows if len(r) > 9 and float(r[7]) >= 0]
            ladder = [(int(r[0]) + 1, float(r[10]), float(r[11])) for r in rows if len(r) > 11 and float(r[10]) > -1.5 and not (r[10] == r[11] == "-1.0000")]
            out[name] = {"segments": isl["seg"], "running": (isl.get("queued") or [""])[0], "failures": isl["failures"],
                         "generations": max([int(r[0]) + 1 for r in rows] or [0]),
                         "yard": yard[-1] if yard else None, "yard_curve": yard[-40:], "ladder_curve": ladder[-40:],
                         "msteps": max([float(r[4]) / 1e6 for r in rows] or [0])}
        lad = self.state.get("crucible_ladder", {}).get("milestones", [])
        lineage = []
        try:
            lines = open(self.crucible_dir("lineage.tsv")).read().splitlines()
            head = lines[0].split("\t")
            lineage = [dict(zip(head, l.split("\t"))) for l in lines[1:]][-30:]
        except (OSError, IndexError):
            pass
        out["_ladder"] = {"milestones": lad, "rungs": [m["id"] for m in self.ladder_rungs(lad)], "lineage": lineage,
                          "kept": len(glob.glob(self.crucible_dir("champions", "*.bin")))}
        save(os.path.join(self.pd, "crucible.json"), out)

    def plan(self, n):
        self.raise_found()
        # The local line: to main when auto mode is on and the credentials
        # allow (Paul Accepts); else its flag stays up.
        if self.state.get("local_line"):
            try:
                if self.auto() and self.push_check()["ok"]:
                    self.resolve()
            except SystemExit as e:
                print("resolve: %s" % e)
            self.resolve_flag()
        # Auto mode: results adopted, the strongest candidate promoted, a
        # prepared promotion approved (inside run_jobs). A promotion moves
        # main, so the builds are asked for after it.
        self.adopt_runs()
        self.candidate_states()
        self.auto_promote()
        self.reread_older()
        self.rotate()
        jobs = self.net_jobs()
        # What a person asked for comes before the standing search.
        jobs += self.run_jobs(n)
        jobs = self.prepare_jobs() + jobs
        self.propagation()
        jobs += self.adopt_jobs()
        self.ticker_scan()
        try:
            self.commits_log()
        except Exception as e:  # the log is for reading; planning goes on
            print("commits: %s" % e, file=sys.stderr)
        # The CPU lanes' share: three queued jobs a CPU machine (the keeper
        # running since 12:28 plans at its half-hourly sync, and a census
        # takes 15 minutes: two a lane between syncs, and one spare). The
        # keeper asks only when its queue is low, and crucible's segments
        # (queued six ahead) and builds waiting behind a long job keep it
        # from looking low: counted against treadcore's share, they left
        # three CPU lanes idle (28 October, 20:40). So the GPU lanes' jobs
        # take none of it, and the share is topped up whatever n says.
        cpu_queued = len([j for j in self.ids("queue") if not j.startswith("x-")])
        want = max(n, 3 * len(self.hosts) - cpu_queued)
        # The families first, a share of the queue: a candidate waits on them.
        jobs += self.family_jobs(max(0, (want - len(jobs) + 1) // 2))
        jobs += self.weight_jobs(max(0, want - len(jobs)))
        # The GPU lanes' own work: it takes no treadcore lane.
        jobs += self.crucible_jobs()
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
        # A plan after every sync, whatever the queue's length: a keeper
        # plans when its queue runs low or runs/ changes, and a prepared
        # promotion or a finished rotation run would otherwise wait for the
        # queue to drain. A file made and removed in runs/ is a change.
        poke = os.path.join(p.q, "runs", ".sync-poke")
        open(poke, "w").close()
        os.remove(poke)
    elif cmd == "why":
        # why RID: a failed promotion's reason, read again from its log.
        st = p.rstate(sys.argv[3])
        if st.get("status") == "failed" and st.get("job"):
            st["why"] = p.promote_failure(st["job"])
            p.rsave(sys.argv[3], st)
            p.keep()
        print(st.get("why", st.get("status")))
    elif cmd == "resolve":
        # resolve: the local line (self-modified promotions) pushed to main.
        p.resolve()
        p.keep()
    elif cmd == "tag":
        # tag RID: an approved promotion's micro version, made now (for one
        # approved before tags were made).
        st = p.rstate(sys.argv[3])
        if st.get("status") != "approved":
            sys.exit("%s is not an approved promotion" % sys.argv[3])
        tag = p.micro_tag(sys.argv[3], st)
        if tag:
            st["tag"] = tag
            p.rsave(sys.argv[3], st)
            p.keep()
        print(tag or "not tagged")
    elif cmd == "now":
        p.now()
        poke = os.path.join(p.q, "runs", ".sync-poke")
        open(poke, "w").close()
        os.remove(poke)
    elif cmd == "runs":
        print(json.dumps(p.runs_status(), indent=1))
    elif cmd == "approve":
        p.approve(sys.argv[3])
    elif cmd == "adopt":
        # adopt FILE WHAT: a weights file as a trial with its two held-out
        # reads, queued at the next top-up.
        jid = p.adopt(open(sys.argv[3]).read(), " ".join(sys.argv[4:]) or os.path.basename(sys.argv[3]))
        p.keep()
        print("%s: queued with its two held-out reads at the next top-up" % jid)
    elif cmd == "push-check":
        st = p.push_check()
        print("push to %s %s: %s" % (st["url"] or st["remote"], st["branch"], "ok" if st["ok"] else "NO: " + st["detail"]))
        sys.exit(0 if st["ok"] else 1)
    elif cmd == "evidence":
        ev = p.evidence()
        for k, v in sorted(ev.items(), key=lambda kv: -(kv[1]["held_mean"] or -99)):
            print("%-58s held %s  families %-9s %s" % (k, " ".join("%+.2f" % h[0] for h in v["held"]), v["families"],
                                                      "current" if v.get("current") else "older defaults"))
    else:
        sys.exit(__doc__)
