#!/usr/bin/env python3
"""fleet-queue: the fleet as one computer -- a queue of jobs, a lane a machine,
a keeper that keeps it all running, and two ways to stop.

    tools/fleet-queue.py DIR start           the keeper, in the foreground
    tools/fleet-queue.py DIR ensure          start the keeper in the background
                                             unless it is already running
    tools/fleet-queue.py DIR stop-adding     the queue drains; what runs finishes;
                                             then the results are collected and
                                             the keeper ends
    tools/fleet-queue.py DIR resume-adding
    tools/fleet-queue.py DIR stop-now        stop every running job now, collect
                                             the results there are, end
    tools/fleet-queue.py DIR shutdown-now    SHUT THE FLEET DOWN: add nothing,
                                             start nothing; each machine finishes
                                             the job in hand, its results and its
                                             ledger come back, then it powers off;
                                             a report says who finished what
    tools/fleet-queue.py DIR sync            gather every machine's record now
    tools/fleet-queue.py DIR new KIND [k=v ...]
                                             ask the planner for a run (a census,
                                             parts, switches, spsa, train ...):
                                             DIR/runs/RID.json; the planner turns
                                             it into jobs at its next top-up
    tools/fleet-queue.py DIR runs            the runs asked for, and where each is
    tools/fleet-queue.py DIR cancel RID      a run takes no more jobs; its queued
                                             jobs leave the queue
    tools/fleet-queue.py DIR approve RID     a prepared promotion becomes main
    tools/fleet-queue.py DIR push-check      can this machine push the project's branch?
    tools/fleet-queue.py DIR flags [--all] [--brief]
                                             what needs attention: the flags not yet
                                             acknowledged (--all: every one)
    tools/fleet-queue.py DIR ack ID|all [--by NAME]
                                             mark a flag (or all) acknowledged
    tools/fleet-queue.py DIR now            do it now: sync, then promote the strongest
                                             current candidate (pushed in auto mode), or
                                             re-read the older ones on today's defaults
    tools/fleet-queue.py DIR status [--json]
    tools/fleet-queue.py DIR add JOB.json    put a job on the queue by hand

DIR/config.json says who works and who plans:

    {"hosts": [{"name": "calber", "target": "user@192.168.8.181", "caps": ["cpu", "gpu"]},
               {"name": "mac", "target": "-", "caps": ["cpu"], "nice": 19}],
     "planner": "python3 -I /path/to/project/scripts/fleet-plan.py DIR",
     "low_water": 6, "poll_s": 10, "max_failures_in_a_row": 5, "sync_s": 1800}

A host may also say how it powers off for shutdown-now ("poweroff": "doas -n
/sbin/poweroff"); one that does not is left running and the report says so.
The keeper runs `PLANNER sync` every sync_s seconds (and at the end), so
every machine's record is gathered without anyone asking.

The planner is the project's: `PLANNER plan N` prints a JSON list of up to N
new jobs, and `PLANNER collect` gathers what the finished jobs found. The
queue knows nothing about what a job is for. A job:

    {"id": "w0042", "kind": "weights", "title": "census of w_near at 1.4",
     "needs": "cpu" | "gpu" | "host:NAME",
     "after": ["r0007", ...],          jobs that must be done first
     "after_host": "prep-{host}",      a job that must be done on the host that
                                       takes this one (its build, say)
     "cmd": "bash, run on the host from $HOME",
     "send": [{"src": "a file here", "dst": "a path on the host"}], before the cmd
     "fetch": "a path under $HOME, copied back into DIR/results/ID/",
     "deliver": {"src": "path on the host", "to": "user@store", "dst": "path on the store"},
     "on_done": "a command run here, after, with JOB_ID and RESULT_DIR set",
     "timeout_s": 7200}

Everything is a file a person can read: queue/, running/HOST/, done/, failed/,
stopped/ (a job each, its JSON), logs/ID.log, results/ID/, lanes/HOST.json
(what each lane is doing), events.jsonl, and the switches STOP_ADDING and
STOP_NOW (and SHUTDOWN), flags/ (what needs a person's attention, each
acknowledged or not), keeper.json (the keeper: pid, start, last plan, last and
next sync, the caffeinate holding a Mac awake). A job is claimed by renaming its file, so two lanes never run one
job. A remote job runs in its own process group, its pid kept on the host,
so `stop-now` can stop all of it.
"""
import glob
import hashlib
import json
import os
import shlex
import signal
import subprocess
import sys
import threading
import time
from base64 import b64encode

SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30"]


def now():
    return time.time()


class Queue:
    def __init__(self, d):
        self.d = os.path.abspath(d)
        for sub in ("queue", "running", "done", "failed", "stopped", "logs", "results", "lanes"):
            os.makedirs(os.path.join(self.d, sub), exist_ok=True)
        self.lock = threading.Lock()

    # ---- files ---------------------------------------------------------
    def p(self, *a):
        return os.path.join(self.d, *a)

    def config(self):
        with open(self.p("config.json")) as f:
            return json.load(f)

    def event(self, what, **kw):
        line = json.dumps({"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "event": what, **kw})
        with self.lock, open(self.p("events.jsonl"), "a") as f:
            f.write(line + "\n")

    def jobs_in(self, sub):
        out = []
        base = self.p(sub)
        for root, _, files in os.walk(base):
            for fn in sorted(files):
                if fn.endswith(".json"):
                    try:
                        with open(os.path.join(root, fn)) as f:
                            j = json.load(f)
                        j["_file"] = os.path.join(root, fn)
                        out.append(j)
                    except (OSError, ValueError):
                        pass
        return out

    def ids_in(self, sub):
        return {j["id"] for j in self.jobs_in(sub)}

    def known(self):
        return set().union(*(self.ids_in(s) for s in ("queue", "running", "done", "failed", "stopped")))

    def add(self, job):
        if not job.get("id") or any(c in job["id"] for c in "/ ."):
            raise ValueError("a job needs an id of letters, digits, - and _")
        if job["id"] in self.known():
            return False
        # The file name orders the queue: the planner's order, kept. A job
        # marked "priority": "high" (what a person asked for) goes ahead of
        # the standing work: its number is set back a few years.
        seq = int(now() * 1000) - (10 ** 11 if job.get("priority") == "high" else 0)
        tmp = self.p("queue", ".%s.tmp" % job["id"])
        with open(tmp, "w") as f:
            json.dump(job, f, indent=1)
        os.rename(tmp, self.p("queue", "%d-%s.json" % (seq, job["id"])))
        return True

    def flag(self, name):
        return os.path.exists(self.p(name))

    def notice(self, kind, text, key=None, progress=False):
        """Raise a flag: something a person (or the agent working with them)
        should look at. One a key: a key already flagged is not raised again.
        flags/SEQ-KIND.json, until acknowledged (ack). A progress record
        (progress=True: a candidate found, say) is news, not a question: it
        is shown (progress()) and never waits for anyone."""
        d = self.p("flags")
        os.makedirs(d, exist_ok=True)
        key = key or "%s:%s" % (kind, text)
        for f in os.listdir(d):
            try:
                if json.load(open(os.path.join(d, f))).get("key") == key:
                    return False
            except (OSError, ValueError):
                pass
        seq = int(now() * 1000)
        fid = "%d-%s" % (seq, kind)
        with open(os.path.join(d, ".%s.tmp" % fid), "w") as f:
            json.dump({"id": fid, "t": time.strftime("%Y-%m-%dT%H:%M:%S"), "kind": kind, "text": text, "key": key, "ack": None,
                       "progress": bool(progress)}, f)
        os.rename(os.path.join(d, ".%s.tmp" % fid), os.path.join(d, fid + ".json"))
        self.event("flag", kind=kind, text=text)
        return True

    def flags(self, every=False):
        out = []
        for f in sorted(glob.glob(self.p("flags", "*.json"))):
            try:
                j = json.load(open(f))
            except (OSError, ValueError):
                continue
            if every or not (j.get("ack") or j.get("progress")):
                out.append(j)
        return out

    def progress(self):
        """The progress records, oldest first: what the fleet has found."""
        out = []
        for f in sorted(glob.glob(self.p("flags", "*.json"))):
            try:
                j = json.load(open(f))
            except (OSError, ValueError):
                continue
            if j.get("progress"):
                out.append(j)
        return out

    def ack(self, fid, by):
        n = 0
        for f in sorted(glob.glob(self.p("flags", "*.json"))):
            j = json.load(open(f))
            if not j.get("ack") and not j.get("progress") and (fid == "all" or j["id"] == fid):
                j["ack"] = {"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "by": by}
                with open(f + ".tmp", "w") as out:
                    json.dump(j, out)
                os.replace(f + ".tmp", f)
                n += 1
        if n:
            self.event("acknowledged", flag=fid, by=by, count=n)
        return n


# ---- running a job on a host --------------------------------------------------

def remote(target, script, timeout=None, stdin=None, capture=True):
    """Run `script` under bash on the host (or here for "-"), sent as base64 so
    no quoting is read twice, whatever the host's login shell."""
    b = b64encode(script.encode()).decode()
    cmd = ["bash", "-c", script] if target == "-" else SSH + [target, 'exec bash -c "$(echo %s | base64 -d)"' % b]
    return subprocess.run(cmd, input=stdin, capture_output=capture, text=stdin is None or isinstance(stdin, str),
                          timeout=timeout)


def run_job(q, host, job, log):
    """The job's command, in its own process group on the host, its pid kept
    in ~/.fleet-queue/ID.pid so stop-now can stop the whole group."""
    jid = job["id"]
    body = job["cmd"]
    # A host's "nice" (config.json) lowers the whole job, its children too.
    nice = "renice -n %d -p $$ >/dev/null 2>&1\n" % host["nice"] if host.get("nice") else ""
    wrapper = nice + (
        "mkdir -p ~/.fleet-queue; cd ~; set -m\n"
        "( %s ) &\n"
        "echo $! > ~/.fleet-queue/%s.pid\n"
        "wait $!; rc=$?; rm -f ~/.fleet-queue/%s.pid; exit $rc\n"
    ) % (body, jid, jid)
    b = b64encode(wrapper.encode()).decode()
    cmd = ["bash", "-c", wrapper] if host["target"] == "-" else SSH + [host["target"], 'exec bash -c "$(echo %s | base64 -d)"' % b]
    with open(log, "a") as out:
        out.write("$ %s\n" % job.get("cmd", "")[:2000])
        out.flush()
        p = subprocess.Popen(cmd, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)
        q.procs[jid] = (p, host)
        try:
            return p.wait(timeout=job.get("timeout_s", 4 * 3600))
        except subprocess.TimeoutExpired:
            stop_remote(host, jid)
            p.kill()
            out.write("timed out after %s s\n" % job.get("timeout_s"))
            return 124
        finally:
            q.procs.pop(jid, None)


def stop_remote(host, jid):
    script = "f=~/.fleet-queue/%s.pid; [ -f $f ] && kill -TERM -$(cat $f) 2>/dev/null; sleep 2; [ -f $f ] && kill -KILL -$(cat $f) 2>/dev/null; rm -f $f; true" % jid
    try:
        remote(host["target"], script, timeout=30)
    except (subprocess.TimeoutExpired, OSError):
        pass


def send(q, host, job, log):
    """Files from here to the host, before the job runs."""
    for f in job.get("send", []):
        src, dst = os.path.expanduser(f["src"]), f["dst"]
        with open(src, "rb") as data:
            body = data.read()
        r = remote(host["target"], "mkdir -p $(dirname %s) && cat > %s" % (dst, dst), timeout=600, stdin=body)
        with open(log, "a") as out:
            out.write("sent %s -> %s (%d bytes, %s)\n" % (src, dst, len(body), "ok" if r.returncode == 0 else "FAILED"))
        if r.returncode != 0:
            return False
    return True


def fetch(q, host, job, log):
    """A path under the host's $HOME, copied into results/ID/ with tar."""
    rel = job["fetch"]
    dest = q.p("results", job["id"])
    os.makedirs(dest, exist_ok=True)
    src = "tar -C ~ -cf - %s" % shlex.quote(rel)
    a = subprocess.Popen(["bash", "-c", src] if host["target"] == "-" else SSH + [host["target"], src], stdout=subprocess.PIPE)
    b = subprocess.run(["tar", "-C", dest, "-xf", "-"], stdin=a.stdout, capture_output=True)
    a.wait()
    with open(log, "a") as out:
        out.write("fetched %s -> %s (%s)\n" % (rel, dest, "ok" if a.returncode == 0 and b.returncode == 0 else "FAILED"))
    return a.returncode == 0 and b.returncode == 0


def deliver(q, host, job, log):
    """A file from the host to the store, hashed as it passes (read once),
    written as .part, its hash read again on the store, then renamed."""
    d = job["deliver"]
    src, to, dst = d["src"], d["to"], d["dst"]
    h = hashlib.sha256()
    n = 0
    if host["target"] == to:
        r = remote(to, "sha256sum %s | cut -c1-64; mkdir -p $(dirname %s); mv %s %s" % (src, dst, src, dst), timeout=600)
        ok = r.returncode == 0
        with open(log, "a") as out:
            out.write("delivered in place on the store: %s (%s)\n" % (dst, "ok" if ok else "FAILED"))
        return ok
    reader = (subprocess.Popen(["bash", "-c", "cat %s" % src], stdout=subprocess.PIPE) if host["target"] == "-"
              else subprocess.Popen(SSH + [host["target"], "cat %s" % src], stdout=subprocess.PIPE))
    writer = subprocess.Popen(SSH + [to, "mkdir -p $(dirname %s) && cat > %s.part" % (dst, dst)], stdin=subprocess.PIPE)
    while True:
        chunk = reader.stdout.read(1 << 20)
        if not chunk:
            break
        h.update(chunk)
        n += len(chunk)
        writer.stdin.write(chunk)
    writer.stdin.close()
    reader.wait()
    writer.wait()
    there = remote(to, "sha256sum %s.part | cut -c1-64" % dst, timeout=600).stdout.strip()
    ok = reader.returncode == 0 and writer.returncode == 0 and there == h.hexdigest()
    with open(log, "a") as out:
        out.write("delivered %d bytes to %s:%s, sha256 %s %s\n" % (n, to, dst, h.hexdigest()[:16], "ok" if ok else "DIFFERS: %s" % there[:16]))
    if ok:
        remote(to, "mv %s.part %s" % (dst, dst), timeout=60)
        remote(host["target"], "rm -f %s" % src, timeout=60)
    return ok


# ---- a lane -------------------------------------------------------------------

def fits(host, job):
    need = job.get("needs", "cpu")
    if need.startswith("host:"):
        return need[5:] == host["name"]
    return need in host.get("caps", ["cpu"])


def lane(q, host, alive):
    name = host["name"]
    os.makedirs(q.p("running", name), exist_ok=True)

    def say(state, job=None):
        tmp = q.p("lanes", ".%s.tmp" % name)
        with open(tmp, "w") as f:
            json.dump({"host": name, "state": state, "job": job and job["id"], "kind": job and job.get("kind"),
                       "title": job and job.get("title"), "since": now()}, f)
        os.rename(tmp, q.p("lanes", name + ".json"))

    say("idle")
    while alive.is_set() and not q.flag("STOP_NOW"):
        if q.flag("SHUTDOWN"):
            power_off(q, host, say)
            return
        # A host taken out of config.json leaves: its lane ends after the job
        # in hand, so a host can be removed while the queue runs.
        if name not in [h["name"] for h in q.config()["hosts"]]:
            q.event("lane left", host=name, why="not in config.json")
            say("removed")
            return
        # A host that cannot be reached waits, probed each minute, and takes
        # nothing: a machine that is down must not fail the queue's jobs.
        if not reachable(host):
            q.event("host unreachable", host=name)
            q.notice("host", "%s cannot be reached: its lane waits" % name, key="host:%s:%s" % (name, time.strftime("%Y-%m-%d %H")))
            say("unreachable")
            while (alive.is_set() and not q.flag("STOP_NOW") and not reachable(host)
                   and name in [h["name"] for h in q.config()["hosts"]]):
                for _ in range(20):
                    if not alive.is_set() or q.flag("STOP_NOW"):
                        break
                    time.sleep(3)
            if name in [h["name"] for h in q.config()["hosts"]]:
                q.event("host back", host=name)
            continue
        done, failed = q.ids_in("done"), q.ids_in("failed") | q.ids_in("stopped")
        picked = None
        for j in q.jobs_in("queue"):
            if not fits(host, j):
                continue
            after = j.get("after", [])
            if any(a in failed for a in after):
                # It cannot run: say so, and why.
                try:
                    os.rename(j["_file"], q.p("failed", os.path.basename(j["_file"])))
                    q.event("failed", job=j["id"], why="a job it waited for failed")
                except OSError:
                    pass
                continue
            ah = j.get("after_host")
            if ah and ah.format(host=name) not in done:
                continue
            if all(a in done for a in after):
                try:
                    claimed = q.p("running", name, os.path.basename(j["_file"]))
                    os.rename(j["_file"], claimed)
                    j["_file"] = claimed
                    picked = j
                    break
                except OSError:
                    continue  # another lane took it
        if not picked:
            say("idle")
            time.sleep(3)
            continue
        try:
            work(q, host, picked, say)
            if q.flag("SHUTDOWN"):
                power_off(q, host, say)
                return
        except Exception as e:  # the job fails; the lane goes on
            q.event("lane error", host=name, job=picked["id"], detail=repr(e)[:300])
            try:
                os.rename(picked["_file"], q.p("failed", os.path.basename(picked["_file"])))
            except OSError:
                pass
    say("stopped")


def power_off(q, host, say):
    """Shutdown: the host's record comes back (PLANNER sync HOST), then it
    powers off by its own command; the lane ends. What it did is reported."""
    name = host["name"]
    planner = q.config().get("planner")
    if planner and reachable(host):
        say("reporting")
        r = subprocess.run(["bash", "-c", "%s sync %s" % (planner, name)], capture_output=True, text=True, timeout=1800)
        q.event("reported", host=name, detail=(r.stdout or r.stderr)[-300:])
    cmd = host.get("poweroff")
    if not cmd:
        q.event("left running", host=name, why="no poweroff in config.json")
        say("left running")
        return
    if not reachable(host):
        q.event("powered off", host=name, detail="already unreachable")
        say("off")
        return
    # The connection drops as the machine goes down: ssh's 255 is expected.
    try:
        r = remote(host["target"], "nohup sh -c 'sleep 2; %s' >/dev/null 2>&1 &" % cmd, timeout=60)
        ok = r.returncode in (0, 255)
        detail = (r.stderr or "").strip()[-200:]
    except subprocess.TimeoutExpired:
        ok, detail = True, "the connection hung as it went down"
    time.sleep(10)
    if ok and not reachable(host):
        q.event("powered off", host=name)
        say("off")
    else:
        q.event("poweroff failed", host=name, detail=detail or "it still answers")
        say("poweroff failed")


def reachable(host):
    """ssh answers (always, for this machine)."""
    if host["target"] == "-":
        return True
    try:
        return remote(host["target"], "true", timeout=30).returncode == 0
    except (subprocess.TimeoutExpired, OSError):
        return False


def work(q, host, j, say):
    name = host["name"]
    if True:
        log = q.p("logs", j["id"] + ".log")
        say("running", j)
        q.event("started", job=j["id"], host=name, kind=j.get("kind"))
        t0 = now()
        rc = run_job(q, host, j, log) if send(q, host, j, log) else 1
        ok = rc == 0
        if ok and j.get("deliver"):
            say("delivering", j)
            ok = deliver(q, host, j, log)
        if ok and j.get("fetch"):
            say("fetching", j)
            ok = fetch(q, host, j, log)
        if q.flag("STOP_NOW") and not ok:
            dest = "stopped"
        elif not ok and rc == 255 and not reachable(host):
            # ssh's own failure, and the host is gone: not the job's fault.
            # It goes back on the queue for any host, its log kept.
            with open(log, "a") as out:
                out.write("the host went away (ssh 255): the job goes back on the queue\n")
            os.rename(j["_file"], q.p("queue", os.path.basename(j["_file"])))
            q.event("requeued", job=j["id"], host=name, why="host unreachable")
            return
        else:
            dest = "done" if ok else "failed"
        j2 = {k: v for k, v in j.items() if k != "_file"}
        j2.update({"host": name, "rc": rc, "seconds": round(now() - t0, 1), "ended": time.strftime("%Y-%m-%dT%H:%M:%S")})
        with open(j["_file"], "w") as f:
            json.dump(j2, f, indent=1)
        os.rename(j["_file"], q.p(dest, os.path.basename(j["_file"])))
        q.event(dest, job=j["id"], host=name, kind=j.get("kind"), seconds=j2["seconds"])
        # The breaker: failures in a row, any host, any kind.
        with q.lock:
            q.streak = q.streak + 1 if dest == "failed" else 0
        if dest == "done" and j.get("on_done"):
            env = dict(os.environ, JOB_ID=j["id"], RESULT_DIR=q.p("results", j["id"]))
            with open(log, "a") as out:
                subprocess.run(["bash", "-c", j["on_done"]], env=env, stdout=out, stderr=subprocess.STDOUT)


# ---- the keeper -----------------------------------------------------------------

def keeper(q):
    cfg = q.config()
    with open(q.p("keeper.pid"), "w") as f:
        f.write(str(os.getpid()))
    q.procs = {}
    q.streak = 0
    # Jobs a keeper that died left running go back on the queue: their lanes
    # died with it.
    for j in q.jobs_in("running"):
        os.rename(j["_file"], q.p("queue", os.path.basename(j["_file"])))
        q.event("requeued", job=j["id"], why="the keeper before this one died while it ran")
    q.event("keeper started", pid=os.getpid())
    # A Mac running the keeper stays awake exactly while it runs: caffeinate
    # waits on the keeper's pid and ends with it (config "keep_awake": false
    # to leave the Mac to its own settings).
    awake = None
    if sys.platform == "darwin" and cfg.get("keep_awake", True):
        try:
            awake = subprocess.Popen(["caffeinate", "-dimsu", "-w", str(os.getpid())], stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True).pid
        except OSError:
            awake = None
    life = {"pid": os.getpid(), "started": time.strftime("%Y-%m-%dT%H:%M:%S"), "awake": awake,
            "last_plan": None, "last_sync": None, "sync_s": cfg.get("sync_s", 1800)}

    def tell():
        with open(q.p(".keeper.json.tmp"), "w") as f:
            json.dump(life, f)
        os.replace(q.p(".keeper.json.tmp"), q.p("keeper.json"))
    alive = threading.Event()
    alive.set()
    threads = {}
    planner = cfg.get("planner")
    low = cfg.get("low_water", 2 * len(cfg["hosts"]))
    poll = cfg.get("poll_s", 10)

    def plan(n):
        r = subprocess.run(["bash", "-c", "%s plan %d" % (planner, n)], capture_output=True, text=True, timeout=600)
        if r.returncode != 0:
            q.event("planner failed", detail=(r.stderr or r.stdout)[-400:])
            q.notice("planner", "the planner failed: " + (r.stderr or r.stdout).strip()[-200:])
            return
        life["last_plan"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        added = 0
        for job in json.loads(r.stdout or "[]"):
            if q.add(job):
                added += 1
        if added:
            q.event("planned", added=added)

    def collect():
        if planner:
            r = subprocess.run(["bash", "-c", "%s collect" % planner], capture_output=True, text=True, timeout=3600)
            q.event("collected", detail=(r.stdout or r.stderr)[-400:])

    syncing = threading.Lock()

    def sync():
        # On its own thread, one at a time: gathering is slow and must not
        # hold the keeper.
        if not planner or not syncing.acquire(blocking=False):
            return
        try:
            r = subprocess.run(["bash", "-c", "%s sync" % planner], capture_output=True, text=True, timeout=3600)
            q.event("synced" if r.returncode == 0 else "sync failed", detail=(r.stdout or r.stderr)[-400:])
            life["last_sync"] = time.strftime("%Y-%m-%dT%H:%M:%S")
            if r.returncode != 0:
                q.notice("sync", "the sync failed: " + (r.stderr or r.stdout).strip()[-200:])
        except subprocess.TimeoutExpired:
            q.event("sync failed", detail="timed out")
        finally:
            syncing.release()

    sync_s = cfg.get("sync_s", 1800)
    last_sync = 0.0
    # A run asked for (DIR/runs/) is planned at once, not at the low-water mark.
    runs_seen = 0.0
    planned_at = 0.0
    plan_s = cfg.get("plan_s", 300)

    try:
        while True:
            with open(q.p("keeper.beat"), "w") as f:
                f.write(str(now()))
            life["beat"] = time.strftime("%Y-%m-%dT%H:%M:%S")
            life["next_sync"] = time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(last_sync + sync_s)) if last_sync and sync_s else None
            tell()
            if q.flag("SHUTDOWN"):
                # Each lane powers its host off after the job in hand; when
                # none is left the keeper reports and ends.
                if not any(t.is_alive() for t in threads.values()) and not q.jobs_in("running"):
                    collect()
                    report = shutdown_report(q)
                    q.event("shut down", detail=report["summary"])
                    q.notice("shutdown", "the fleet shut down: " + report["summary"])
                    break
                time.sleep(poll)
                continue
            if planner and sync_s and now() - last_sync > sync_s:
                last_sync = now()
                threading.Thread(target=sync, daemon=True).start()
            # Keep a lane alive for every host; one that died is started again.
            for h in q.config()["hosts"]:
                t = threads.get(h["name"])
                if t is None or not t.is_alive():
                    if t is not None:
                        q.event("lane restarted", host=h["name"])
                    t = threading.Thread(target=lane, args=(q, h, alive), daemon=True)
                    t.start()
                    threads[h["name"]] = t
            if q.flag("STOP_NOW"):
                q.event("stopping now")
                alive.clear()
                for jid, (p, host) in list(q.procs.items()):
                    stop_remote(host, jid)
                    p.terminate()
                for t in threads.values():
                    t.join(timeout=60)
                # What was running goes to stopped/, its log kept.
                for j in q.jobs_in("running"):
                    os.rename(j["_file"], q.p("stopped", os.path.basename(j["_file"])))
                collect()
                q.event("stopped")
                break
            # THE BREAKER. A job that fails at once empties the queue as fast
            # as the planner fills it; after this many failures in a row the
            # keeper stops adding and says so (converge stops after three).
            limit = cfg.get("max_failures_in_a_row", 5)
            if q.streak >= limit and not q.flag("STOP_ADDING"):
                open(q.p("STOP_ADDING"), "w").close()
                q.event("stop adding", why="%d jobs failed in a row: read logs/, fix, then resume-adding" % q.streak)
                q.notice("breaker", "%d jobs failed in a row: the queue stopped adding (read logs/, fix, then Resume adding)" % q.streak)
            queued = len(q.jobs_in("queue"))
            running = len(q.jobs_in("running"))
            if q.flag("STOP_ADDING"):
                if queued == 0 and running == 0:
                    collect()
                    q.event("drained")
                    alive.clear()
                    break
            elif planner and (queued < low or runs_mtime(q) > runs_seen or time.time() - planned_at > plan_s):
                # Also every plan_s (300 s) whatever the queue's length, so
                # auto mode's promotions and the rotation never wait for it
                # to drain.
                runs_seen = runs_mtime(q)
                planned_at = time.time()
                try:
                    plan(max(2 * low - queued, low))
                except (subprocess.TimeoutExpired, ValueError) as e:
                    q.event("planner failed", detail=str(e))
            time.sleep(poll)
    finally:
        alive.clear()
        try:
            os.remove(q.p("keeper.pid"))
        except OSError:
            pass


def runs_mtime(q):
    try:
        return os.stat(q.p("runs")).st_mtime
    except OSError:
        return 0.0


def shutdown_report(q):
    """Who finished what, and what was left: DIR/shutdown.json."""
    ev = [json.loads(l) for l in open(q.p("events.jsonl"))]
    t0 = max((e["t"] for e in ev if e.get("event") == "shutdown now"), default="")
    hosts = {}
    for h in q.config()["hosts"]:
        mine = [e for e in ev if e.get("host") == h["name"]]
        last = [e for e in mine if e.get("event") in ("done", "failed", "stopped")]
        end = [e for e in mine if e["t"] >= t0 and e.get("event") in ("powered off", "left running", "poweroff failed")]
        hosts[h["name"]] = {"last_job": last[-1]["job"] if last else None, "last_result": last[-1]["event"] if last else None,
                            "finished_at": last[-1]["t"] if last else None,
                            "power": end[-1]["event"] if end else "unknown",
                            "reported": any(e.get("event") == "reported" and e["t"] >= t0 for e in mine)}
    left = [j["id"] for j in q.jobs_in("queue")]
    report = {"t": time.strftime("%Y-%m-%dT%H:%M:%S"), "asked": t0, "hosts": hosts, "left_on_the_queue": left,
              "summary": "; ".join("%s: %s, %s" % (n, h["last_job"] or "nothing", h["power"]) for n, h in hosts.items())
              + "; %d jobs never started" % len(left)}
    with open(q.p("shutdown.json"), "w") as f:
        json.dump(report, f, indent=1)
    return report


def keeper_alive(q):
    try:
        pid = int(open(q.p("keeper.pid")).read())
        os.kill(pid, 0)
        return pid
    except (OSError, ValueError):
        return None


def status(q):
    cfg = q.config()
    lanes = []
    for h in cfg["hosts"]:
        try:
            with open(q.p("lanes", h["name"] + ".json")) as f:
                l = json.load(f)
        except (OSError, ValueError):
            l = {"host": h["name"], "state": "not started"}
        if l.get("since"):
            l["age_s"] = int(now() - l["since"])
        l["caps"] = h.get("caps", ["cpu"])
        lanes.append(l)
    counts = {}
    for sub in ("queue", "running", "done", "failed", "stopped"):
        for j in q.jobs_in(sub):
            k = j.get("kind", "job")
            counts.setdefault(k, {}).setdefault(sub, 0)
            counts[k][sub] += 1
    pid = keeper_alive(q)
    state = ("stopping now" if q.flag("STOP_NOW") and pid else
             "shutting down: each machine finishes, reports, powers off" if q.flag("SHUTDOWN") and pid else
             "shut down" if q.flag("SHUTDOWN") and not pid else
             "stopped" if not pid else
             "draining: adding nothing" if q.flag("STOP_ADDING") else "running, adding")
    recent = sorted(q.jobs_in("done") + q.jobs_in("failed") + q.jobs_in("stopped"), key=lambda j: j.get("ended", ""))[-12:]
    upcoming = q.jobs_in("queue")[:8]
    events = []
    try:
        events = [json.loads(l) for l in open(q.p("events.jsonl")).readlines()[-8:]]
    except OSError:
        pass
    shutdown = None
    try:
        shutdown = json.load(open(q.p("shutdown.json"))) if q.flag("SHUTDOWN") else None
    except (OSError, ValueError):
        pass
    sync = None
    try:
        sync = json.load(open(os.path.join(os.path.dirname(q.d.rstrip("/")), "..", "ledger", "sync.json")))
    except (OSError, ValueError):
        pass
    try:
        keeper_info = json.load(open(q.p("keeper.json"))) if pid else None
    except (OSError, ValueError):
        keeper_info = None
    return {"dir": q.d, "state": state, "keeper_pid": pid, "adding": not q.flag("STOP_ADDING"),
            "keeper": keeper_info, "flags": q.flags(), "progress": q.progress()[-200:],
            "auto": q.config().get("auto", True) is not False,
            "rotation": q.config().get("rotation", True) is not False,
            "shutdown": shutdown, "sync": sync,
            "counts": counts, "lanes": lanes,
            "recent": [{"id": j["id"], "kind": j.get("kind"), "title": j.get("title"), "host": j.get("host"),
                        "seconds": j.get("seconds"), "ok": j["_file"].startswith(q.p("done"))} for j in recent],
            "upcoming": [{"id": j["id"], "kind": j.get("kind"), "title": j.get("title"), "needs": j.get("needs")} for j in upcoming],
            "events": events}


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__)
    q = Queue(argv[0])
    cmd = argv[1]
    if cmd == "start":
        if keeper_alive(q):
            sys.exit("a keeper is already running (pid %s)" % keeper_alive(q))
        for f in ("STOP_NOW", "SHUTDOWN"):
            try:
                os.remove(q.p(f))
            except OSError:
                pass
        keeper(q)
    elif cmd == "ensure":
        pid = keeper_alive(q)
        if pid:
            print("the keeper is running (pid %d)" % pid)
            return
        log = open(q.p("keeper.log"), "a")
        p = subprocess.Popen([sys.executable, os.path.abspath(__file__), q.d, "start"], stdout=log, stderr=log,
                             stdin=subprocess.DEVNULL, start_new_session=True)
        print("the keeper started (pid %d)" % p.pid)
    elif cmd == "stop-adding":
        open(q.p("STOP_ADDING"), "w").close()
        q.event("stop adding")
        print("adding stopped: the queue drains, then results are collected and the keeper ends")
    elif cmd == "resume-adding":
        for f in ("STOP_ADDING",):
            try:
                os.remove(q.p(f))
            except OSError:
                pass
        q.event("resume adding")
        print("adding again")
    elif cmd == "stop-now":
        open(q.p("STOP_NOW"), "w").close()
        q.event("stop now")
        print("stopping now: running jobs are stopped, results collected, the keeper ends")
    elif cmd == "shutdown-now":
        open(q.p("STOP_ADDING"), "w").close()
        open(q.p("SHUTDOWN"), "w").close()
        q.event("shutdown now")
        print("shutting the fleet down: nothing new starts; each machine finishes its job, reports, then powers off")
    elif cmd == "sync":
        cfg = q.config()
        r = subprocess.run(["bash", "-c", "%s sync" % cfg["planner"]], text=True)
        sys.exit(r.returncode)
    elif cmd == "now":
        # Do it now: the planner syncs, then promotes the strongest current
        # candidate (tested, its families read, pushed in auto mode), or
        # re-reads the strongest older ones on today's defaults.
        cfg = q.config()
        r = subprocess.run(["bash", "-c", "%s now" % cfg["planner"]], text=True)
        sys.exit(r.returncode)
    elif cmd == "new":
        if len(argv) < 3:
            sys.exit("new KIND [key=value ...]")
        os.makedirs(q.p("runs"), exist_ok=True)
        n = 1 + max([int(f[1:5]) for f in os.listdir(q.p("runs")) if f[:1] == "r" and f[1:5].isdigit()] or [0])
        rid = "r%04d" % n
        params = dict(kv.split("=", 1) for kv in argv[3:] if "=" in kv)
        with open(q.p("runs", rid + ".json"), "w") as f:
            json.dump({"kind": argv[2], "params": params, "asked": time.strftime("%Y-%m-%dT%H:%M:%S")}, f, indent=1)
        q.event("run asked", run=rid, kind=argv[2], params=params)
        print("%s: %s asked; the planner turns it into jobs at its next top-up" % (rid, argv[2]))
    elif cmd == "runs":
        r = subprocess.run(["bash", "-c", "%s runs" % q.config()["planner"]], text=True)
        sys.exit(r.returncode)
    elif cmd == "approve":
        # Paul's word on a prepared promotion: the planner moves main to it.
        r = subprocess.run(["bash", "-c", "%s approve %s" % (q.config()["planner"], shlex.quote(argv[2]))], text=True)
        q.event("approved" if r.returncode == 0 else "approve failed", run=argv[2])
        sys.exit(r.returncode)
    elif cmd == "flags":
        fl = q.flags(every="--all" in argv)
        if "--json" in argv:
            print(json.dumps(fl, indent=1))
        elif "--brief" in argv:
            if fl:
                print("orrery %s: %d flag(s) not acknowledged" % (os.path.basename(q.d.rstrip("/")), len(fl)))
                for f in fl:
                    print("  [%s] %s %s: %s" % (f["id"], f["t"][11:16], f["kind"], f["text"]))
        else:
            for f in fl:
                print("%s %s %-10s %s%s" % (f["id"], f["t"], f["kind"], f["text"], "  (acknowledged by %s)" % f["ack"]["by"] if f.get("ack") else ""))
    elif cmd in ("auto", "rotation"):
        # auto on|off: whether candidates promote themselves; rotation on|off:
        # whether auto mode asks one of each kind of run in turn. The plan
        # reads config.json's "auto" and "rotation", each on unless false.
        key = cmd
        if len(argv) < 3 or argv[2] not in ("on", "off"):
            print("%s is %s" % (key, "on" if q.config().get(key, True) is not False else "off"))
        else:
            cfg = json.load(open(q.p("config.json")))
            cfg[key] = argv[2] == "on"
            with open(q.p("config.json.tmp"), "w") as f:
                json.dump(cfg, f, indent=1)
            os.replace(q.p("config.json.tmp"), q.p("config.json"))
            q.event(key, on=cfg[key])
            print("%s is %s" % (key, argv[2]))
    elif cmd == "progress":
        for f in q.progress():
            print("%s %-10s %s" % (f["t"], f["kind"], f["text"]))
    elif cmd == "ack":
        by = argv[argv.index("--by") + 1] if "--by" in argv else os.environ.get("USER", "someone")
        n = q.ack(argv[2], by)
        print("%d flag(s) acknowledged" % n)
    elif cmd == "push-check":
        # Can this coordinator push the project's branch (Approve is a push)?
        r = subprocess.run(["bash", "-c", "%s push-check" % q.config()["planner"]], text=True)
        sys.exit(r.returncode)
    elif cmd == "cancel":
        rid = argv[2]
        try:
            os.rename(q.p("runs", rid + ".json"), q.p("runs", rid + ".cancelled"))
        except OSError:
            sys.exit("no run %s" % rid)
        n = 0
        for j in q.jobs_in("queue"):
            if j["id"].startswith(rid + "-"):
                os.rename(j["_file"], q.p("stopped", os.path.basename(j["_file"])))
                n += 1
        q.event("run cancelled", run=rid, dropped=n)
        print("%s cancelled: %d queued jobs left the queue; running ones finish" % (rid, n))
    elif cmd == "status":
        s = status(q)
        if "--json" in argv:
            print(json.dumps(s, indent=1))
        else:
            print("%s  (keeper %s)" % (s["state"], s["keeper_pid"] or "not running"))
            for k, c in sorted(s["counts"].items()):
                print("  %-10s %s" % (k, "  ".join("%s %d" % (x, c.get(x, 0)) for x in ("queue", "running", "done", "failed", "stopped"))))
            for l in s["lanes"]:
                print("  %-12s %-10s %s" % (l["host"], l.get("state"), l.get("title") or ""))
    elif cmd == "add":
        for path in argv[2:]:
            with open(path) as f:
                jobs = json.load(f)
            for j in (jobs if isinstance(jobs, list) else [jobs]):
                print(("added " if q.add(j) else "already known ") + j["id"])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
