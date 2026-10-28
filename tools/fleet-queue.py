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
    tools/fleet-queue.py DIR status [--json]
    tools/fleet-queue.py DIR add JOB.json    put a job on the queue by hand

DIR/config.json says who works and who plans:

    {"hosts": [{"name": "calber", "target": "user@192.168.8.181", "caps": ["cpu", "gpu"]},
               {"name": "mac", "target": "-", "caps": ["cpu"]}],
     "planner": "python3 -I /path/to/project/scripts/fleet-plan.py DIR",
     "low_water": 6, "poll_s": 10}

The planner is the project's: `PLANNER plan N` prints a JSON list of up to N
new jobs, and `PLANNER collect` gathers what the finished jobs found. The
queue knows nothing about what a job is for. A job:

    {"id": "w0042", "kind": "weights", "title": "census of w_near at 1.4",
     "needs": "cpu" | "gpu" | "host:NAME",
     "after": ["r0007", ...],          jobs that must be done first
     "cmd": "bash, run on the host from $HOME",
     "fetch": "a path under $HOME, copied back into DIR/results/ID/",
     "deliver": {"src": "path on the host", "to": "user@store", "dst": "path on the store"},
     "on_done": "a command run here, after, with JOB_ID and RESULT_DIR set",
     "timeout_s": 7200}

Everything is a file a person can read: queue/, running/HOST/, done/, failed/,
stopped/ (a job each, its JSON), logs/ID.log, results/ID/, lanes/HOST.json
(what each lane is doing), events.jsonl, and the switches STOP_ADDING and
STOP_NOW. A job is claimed by renaming its file, so two lanes never run one
job. A remote job runs in its own process group, its pid kept on the host,
so `stop-now` can stop all of it.
"""
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
        # The file name orders the queue: the planner's order, kept.
        seq = int(now() * 1000)
        tmp = self.p("queue", ".%s.tmp" % job["id"])
        with open(tmp, "w") as f:
            json.dump(job, f, indent=1)
        os.rename(tmp, self.p("queue", "%d-%s.json" % (seq, job["id"])))
        return True

    def flag(self, name):
        return os.path.exists(self.p(name))


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
    wrapper = (
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
        except Exception as e:  # the job fails; the lane goes on
            q.event("lane error", host=name, job=picked["id"], detail=repr(e)[:300])
            try:
                os.rename(picked["_file"], q.p("failed", os.path.basename(picked["_file"])))
            except OSError:
                pass
    say("stopped")


def work(q, host, j, say):
    name = host["name"]
    if True:
        log = q.p("logs", j["id"] + ".log")
        say("running", j)
        q.event("started", job=j["id"], host=name, kind=j.get("kind"))
        t0 = now()
        rc = run_job(q, host, j, log)
        ok = rc == 0
        if ok and j.get("deliver"):
            say("delivering", j)
            ok = deliver(q, host, j, log)
        if ok and j.get("fetch"):
            say("fetching", j)
            ok = fetch(q, host, j, log)
        if q.flag("STOP_NOW") and not ok:
            dest = "stopped"
        else:
            dest = "done" if ok else "failed"
        j2 = {k: v for k, v in j.items() if k != "_file"}
        j2.update({"host": name, "rc": rc, "seconds": round(now() - t0, 1), "ended": time.strftime("%Y-%m-%dT%H:%M:%S")})
        with open(j["_file"], "w") as f:
            json.dump(j2, f, indent=1)
        os.rename(j["_file"], q.p(dest, os.path.basename(j["_file"])))
        q.event(dest, job=j["id"], host=name, kind=j.get("kind"), seconds=j2["seconds"])
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
    # Jobs a keeper that died left running go back on the queue: their lanes
    # died with it.
    for j in q.jobs_in("running"):
        os.rename(j["_file"], q.p("queue", os.path.basename(j["_file"])))
        q.event("requeued", job=j["id"], why="the keeper before this one died while it ran")
    q.event("keeper started", pid=os.getpid())
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
            return
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

    try:
        while True:
            with open(q.p("keeper.beat"), "w") as f:
                f.write(str(now()))
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
            queued = len(q.jobs_in("queue"))
            running = len(q.jobs_in("running"))
            if q.flag("STOP_ADDING"):
                if queued == 0 and running == 0:
                    collect()
                    q.event("drained")
                    alive.clear()
                    break
            elif planner and queued < low:
                try:
                    plan(2 * low - queued)
                except (subprocess.TimeoutExpired, ValueError) as e:
                    q.event("planner failed", detail=str(e))
            time.sleep(poll)
    finally:
        alive.clear()
        try:
            os.remove(q.p("keeper.pid"))
        except OSError:
            pass


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
             "stopped" if not pid else
             "draining: adding nothing" if q.flag("STOP_ADDING") else "running, adding")
    recent = sorted(q.jobs_in("done") + q.jobs_in("failed") + q.jobs_in("stopped"), key=lambda j: j.get("ended", ""))[-12:]
    upcoming = q.jobs_in("queue")[:8]
    events = []
    try:
        events = [json.loads(l) for l in open(q.p("events.jsonl")).readlines()[-8:]]
    except OSError:
        pass
    return {"dir": q.d, "state": state, "keeper_pid": pid, "adding": not q.flag("STOP_ADDING"),
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
        for f in ("STOP_NOW",):
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
