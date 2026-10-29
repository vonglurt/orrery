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
    tools/fleet-queue.py DIR health         is it running as it should: the keeper, its code,
                                             its planning and syncing, the lanes, the jobs waiting
    tools/fleet-queue.py DIR plan-now        the keeper asks the planner for work at its next beat
    tools/fleet-queue.py DIR restart         the keeper restarts on the code on disk; the jobs
                                             running go back on the queue and run again
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
        self.restarting = False

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


def ready(host, job, done):
    """The host could start the job now: it fits, and what it waits for
    (its jobs, and its build on this host) is done."""
    ah = job.get("after_host")
    if ah and ah.format(host=host["name"]) not in done:
        return False
    return fits(host, job) and all(a in done for a in job.get("after", []))


def lane_file(q, name):
    try:
        with open(q.p("lanes", name + ".json")) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def starved(q, cfg, idle_s=None):
    """The lanes idle at least idle_s with nothing queued they could start.

    THE QUEUE'S LENGTH IS NOT THE QUESTION. On 28 October it held eleven jobs
    and five CPU lanes sat idle: five were builds for whitewolf, waiting
    behind its six-hour school job, and six were crucible's segments, queued
    ahead for the GPU lanes. Eleven is above the low-water mark, so the
    keeper never asked for more. A lane with nothing it can start is the
    question, whatever else is queued."""
    idle_s = cfg.get("starve_idle_s", 30) if idle_s is None else idle_s
    jobs = q.jobs_in("queue")
    done = q.ids_in("done")
    out = []
    for h in cfg["hosts"]:
        l = lane_file(q, h["name"])
        if l.get("state") == "idle" and now() - l.get("since", now()) >= idle_s and not any(ready(h, j, done) for j in jobs):
            out.append(h["name"])
    return out


def code_id():
    """This file's identity: what a keeper runs, against what is on disk."""
    try:
        with open(os.path.abspath(__file__), "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()[:12]
    except OSError:
        return None


def lane(q, host, alive):
    name = host["name"]
    os.makedirs(q.p("running", name), exist_ok=True)

    said = {}

    def say(state, job=None):
        # "since" is when the lane came to this state: an idle lane says
        # idle every three seconds, and an hour of it must read as an hour.
        jid = job and job["id"]
        if said.get("state") != state or said.get("job") != jid:
            said.update(state=state, job=jid, since=now())
        tmp = q.p("lanes", ".%s.tmp" % name)
        with open(tmp, "w") as f:
            json.dump({"host": name, "state": state, "job": jid, "kind": job and job.get("kind"),
                       "title": job and job.get("title"), "since": said["since"]}, f)
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
            if ready(host, j, done):
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
        elif q.restarting and not ok:
            # The keeper restarts for new code: what it cut short runs again.
            with open(log, "a") as out:
                out.write("the keeper restarted for new code: the job goes back on the queue\n")
            os.rename(j["_file"], q.p("queue", os.path.basename(j["_file"])))
            q.event("requeued", job=j["id"], host=name, why="the keeper restarted for new code")
            return
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
    # A keeper that restarted itself is the same process (exec keeps the
    # pid), so the caffeinate waiting on it still holds: it is kept.
    try:
        inherited = int(os.environ.pop("FLEET_QUEUE_AWAKE", ""))
        os.kill(inherited, 0)
        awake = inherited
    except (ValueError, OSError):
        pass
    if awake is None and sys.platform == "darwin" and cfg.get("keep_awake", True):
        try:
            awake = subprocess.Popen(["caffeinate", "-dimsu", "-w", str(os.getpid())], stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True).pid
        except OSError:
            awake = None
    life = {"pid": os.getpid(), "started": time.strftime("%Y-%m-%dT%H:%M:%S"), "awake": awake,
            "last_plan": None, "last_sync": None, "sync_s": cfg.get("sync_s", 1800),
            # The code it runs: health() sets it against the file on disk.
            "code": code_id(), "file": os.path.abspath(__file__),
            "restart_lose_s": cfg.get("restart_lose_s", 600)}

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
    # A lane idle with nothing it can start asks for work each starve_s,
    # however long the queue (starved()).
    starve_s = cfg.get("starve_s", 60)
    told_hungry = []
    code_seen = (life["code"], os.stat(__file__).st_mtime if os.path.exists(__file__) else 0)

    def restart(why):
        # THE KEEPER RESTARTS ITSELF ON NEW CODE. A keeper started at 12:28
        # ran 12:28's code until 21:20, and the fix made at 14:29 waited for
        # "its next start" all evening. The lanes stop, what they cut short
        # goes back on the queue (work(), q.restarting), and the process
        # execs itself: the same pid, so keeper.pid and the caffeinate hold.
        q.restarting = True
        q.event("keeper restarting", why=why)
        alive.clear()
        for jid, (p, host) in list(q.procs.items()):
            stop_remote(host, jid)
            p.terminate()
        for t in threads.values():
            t.join(timeout=90)
        try:
            os.remove(q.p("RESTART"))
        except OSError:
            pass
        sys.stdout.flush()
        sys.stderr.flush()
        env = dict(os.environ, FLEET_QUEUE_AWAKE=str(awake or ""))
        os.execve(sys.executable, [sys.executable, os.path.abspath(__file__), q.d, "start"], env)

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
            # New code on disk: the keeper restarts when that throws away
            # little (nothing running longer than restart_lose_s, ten
            # minutes: a census or a segment, run again), or at once when a
            # person pressed Restart (DIR/RESTART). A six-hour job is not
            # thrown away for a code change on the keeper's own word.
            try:
                mtime = os.stat(__file__).st_mtime
            except OSError:
                mtime = code_seen[1]
            if mtime != code_seen[1]:
                code_seen = (code_id() or code_seen[0], mtime)
            stale = code_seen[0] != life["code"]
            life["code_on_disk"] = code_seen[0]
            if q.flag("RESTART"):
                restart("Restart was pressed")
            if stale:
                ages = [now() - l.get("since", now()) for l in (lane_file(q, h["name"]) for h in q.config()["hosts"])
                        if l.get("job")]
                if max(ages, default=0) < life["restart_lose_s"]:
                    restart("new code on disk (%s, was %s)" % (code_seen[0], life["code"]))
            queued = len(q.jobs_in("queue"))
            running = len(q.jobs_in("running"))
            hungry = starved(q, cfg) if planner and not q.flag("STOP_ADDING") else []
            life["starved"] = hungry
            if q.flag("STOP_ADDING"):
                if queued == 0 and running == 0:
                    collect()
                    q.event("drained")
                    alive.clear()
                    break
            elif planner and (queued < low or runs_mtime(q) > runs_seen or time.time() - planned_at > plan_s
                              or (hungry and time.time() - planned_at > starve_s)):
                # Also every plan_s (300 s) whatever the queue's length, so
                # auto mode's promotions and the rotation never wait for it
                # to drain; and each starve_s while a lane is idle with
                # nothing it can start (starved()). Plan now touches runs/.
                if hungry and queued >= low and hungry != told_hungry:
                    q.event("planning for idle lanes", hosts=hungry, queued=queued)
                told_hungry = hungry
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


def took(s):
    s = int(s)
    return "%d h %02d m" % (s // 3600, s // 60 % 60) if s >= 3600 else "%d min" % (s // 60) if s >= 60 else "%d s" % s


def stamp_age(t):
    """Seconds since a keeper.json stamp (local time), or None."""
    try:
        return now() - time.mktime(time.strptime(t, "%Y-%m-%dT%H:%M:%S"))
    except (TypeError, ValueError):
        return None


def health(q):
    """Is it running as it should? Each check a level (ok, warn, bad), a name
    and a sentence, for the console's Settings page and `health`. Nothing is
    fixed here: the keeper fixes what it can by itself (plans for idle
    lanes, restarts on new code), and what is left is a button."""
    cfg = q.config()
    pid = keeper_alive(q)
    out = []

    def check(level, name, text):
        out.append({"level": level, "name": name, "text": text})
    info = {}
    if pid:
        try:
            info = json.load(open(q.p("keeper.json")))
        except (OSError, ValueError):
            pass
    lanes = {h["name"]: lane_file(q, h["name"]) for h in cfg["hosts"]}
    running = [(n, l) for n, l in lanes.items() if l.get("job")]
    oldest = max(running, key=lambda r: now() - r[1].get("since", now()), default=None)
    if not pid:
        check("bad", "keeper", "no keeper is running: nothing starts (Start the queue)")
        return out
    check("ok", "keeper", "pid %d since %s" % (pid, (info.get("started") or "?")[11:16]))
    beat = stamp_age(info.get("beat"))
    poll = cfg.get("poll_s", 10)
    if beat is None or beat > max(60, 6 * poll):
        check("bad", "heartbeat", "no beat for %s: the keeper is alive but its loop is stuck" % took(beat or 0))
    else:
        check("ok", "heartbeat", "last beat %s ago" % took(beat))
    disk = code_id()
    lose = info.get("restart_lose_s", 600)
    held = ("; %s's %s is %s in" % (oldest[0], oldest[1].get("title") or oldest[1]["job"], took(now() - oldest[1]["since"]))
            if oldest and now() - oldest[1].get("since", now()) >= lose else "")
    if info.get("code"):
        if info["code"] == disk:
            check("ok", "code", "it runs the code on disk (%s)" % disk)
        else:
            check("warn", "code", "it runs older code (%s, the file is %s): it restarts itself once nothing running "
                  "is older than %s%s, or press Restart the keeper" % (info["code"], disk, took(lose), held))
    else:
        changed = os.stat(__file__).st_mtime
        started = stamp_age(info.get("started"))
        if started is not None and now() - started < changed:
            check("warn", "code", "it started at %s, before the file changed at %s, and is too old to restart "
                  "itself: press Restart the keeper%s" % (info["started"][11:16], time.strftime("%H:%M", time.localtime(changed)), held))
        else:
            check("ok", "code", "it started after the file last changed")
    events = []
    try:
        events = [json.loads(l) for l in open(q.p("events.jsonl")).readlines()[-400:]]
    except (OSError, ValueError):
        pass
    plan_s = cfg.get("plan_s", 300)
    planned = stamp_age(info.get("last_plan"))
    failed = [e for e in events if e.get("event") == "planner failed" and (info.get("last_plan") or "") < e["t"]]
    if failed:
        check("bad", "planning", "the planner failed at %s: %s" % (failed[-1]["t"][11:16], (failed[-1].get("detail") or "")[-160:]))
    elif q.flag("STOP_ADDING"):
        check("warn", "planning", "adding nothing: the queue drains (Resume adding)")
    elif planned is None or planned > 2 * plan_s + 60:
        check("warn", "planning", "last asked the planner %s ago" % (took(planned) if planned is not None else "never"))
    else:
        check("ok", "planning", "last asked the planner %s ago" % took(planned))
    sync_s = cfg.get("sync_s", 1800)
    synced = stamp_age(info.get("last_sync"))
    if not sync_s:
        check("ok", "sync", "sync_s is 0: the keeper gathers nothing by itself")
    elif synced is None or synced > sync_s + 600:
        check("warn", "sync", "last gathered the machines' records %s ago" % (took(synced) if synced is not None else "never"))
    else:
        check("ok", "sync", "last gathered %s ago" % took(synced or 0))
    if info.get("awake") is not None or sys.platform == "darwin":
        try:
            os.kill(int(info.get("awake")), 0)
            check("ok", "awake", "caffeinate %d holds this Mac awake" % info["awake"])
        except (TypeError, ValueError, OSError):
            check("warn" if cfg.get("keep_awake", True) else "ok", "awake", "nothing holds this Mac awake" if cfg.get("keep_awake", True) else "keep_awake is off")
    down = [n for n, l in lanes.items() if l.get("state") == "unreachable"]
    if down:
        check("bad", "machines", "cannot be reached, its lane waiting: %s" % ", ".join(down))
    else:
        check("ok", "machines", "every machine answers")
    hungry = starved(q, cfg)
    if hungry:
        check("warn", "lanes", "idle with nothing they can start: %s (the keeper asks the planner for them each %s)"
              % (", ".join(hungry), took(cfg.get("starve_s", 60))))
    else:
        check("ok", "lanes", "%d of %d running; every idle lane has work it can start" % (len(running), len(lanes)))
    # The jobs no lane can take: needs no machine has, or one machine only
    # (a build for it, say) and that machine an hour into something else.
    done = q.ids_in("done")
    nobody, behind = [], {}
    for j in q.jobs_in("queue"):
        can = [h for h in cfg["hosts"] if fits(h, j)]
        if not can:
            nobody.append(j["id"])
        elif len(can) == 1 and ready(can[0], j, done):
            behind.setdefault(can[0]["name"], []).append(j["id"])
    if nobody:
        check("bad", "jobs", "%d queued that no machine can take (%s): their needs match no lane" % (len(nobody), ", ".join(nobody[:4])))
    waits = ["%d behind %s's %s (%s in)" % (len(v), n, lanes[n].get("title") or lanes[n]["job"], took(now() - lanes[n]["since"]))
             for n, v in sorted(behind.items()) if lanes[n].get("job") and now() - lanes[n].get("since", now()) > 3600]
    if waits:
        check("warn", "jobs", "waiting for one machine, busy over an hour: " + "; ".join(waits))
    elif not nobody:
        check("ok", "jobs", "every queued job has a machine it can run on")
    # The search, as the planner last saw it (plan/search.json): busy lanes
    # are not finding. Widening is the planner's own; a search still dry at
    # three times the stall wants what a planner cannot do, a change to the
    # brain.
    s = {}
    try:
        s = json.load(open(q.p("plan", "search.json")))
    except (OSError, ValueError):
        pass
    if s:
        last = ("the last find %s, %d trials ago" % (s["last_find"], s["since_find"]) if s.get("last_find")
                else "nothing found in %d" % s["since_find"])
        said = "%d trials on today's defaults, %d different (%d repeats not asked); %s" % (
            s.get("reads", 0), s.get("distinct", 0), s.get("repeats", 0), last)
        q_ = s.get("sequential") or {}
        if q_:
            said += "; read on until decided: %d reading, %d to read on, %d dropped, %d passed to the held-out reads%s" % (
                q_.get("reading", 0), q_.get("read on", 0), q_.get("dropped", 0), q_.get("passed", 0),
                ", %d seeds a decision" % q_["seeds_a_decision"] if q_.get("seeds_a_decision") else "")
        stall = s.get("stall", 150)
        if s.get("stalled") and s.get("reads", 0) - s.get("widened_at", s.get("reads", 0)) >= 3 * stall:
            check("warn", "search", said + ": dry %d trials since it widened, the next gain wants a change to the brain itself"
                  % (s["reads"] - s["widened_at"]))
        elif s.get("stalled"):
            check("ok", "search", said + ": widened (bound %s, half of each generation wide trials)" % s.get("bound"))
        else:
            check("ok", "search", said)
    return out


def restart_cost(q):
    """What Restart the keeper throws away: the jobs running, run again."""
    rows = []
    for h in q.config()["hosts"]:
        l = lane_file(q, h["name"])
        if l.get("job"):
            rows.append((now() - l.get("since", now()), l.get("title") or l["job"], h["name"]))
    rows.sort(reverse=True)
    return {"jobs": len(rows), "oldest": ("%s's %s, %s in" % (rows[0][2], rows[0][1], took(rows[0][0]))) if rows else ""}


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
            "events": events, "health": health(q), "restart": restart_cost(q)}


def ensure(q):
    pid = keeper_alive(q)
    if pid:
        print("the keeper is running (pid %d)" % pid)
        return
    log = open(q.p("keeper.log"), "a")
    p = subprocess.Popen([sys.executable, os.path.abspath(__file__), q.d, "start"], stdout=log, stderr=log,
                         stdin=subprocess.DEVNULL, start_new_session=True)
    print("the keeper started (pid %d)" % p.pid)


def restart_keeper(q):
    """Restart the keeper on the code on disk. A keeper that can restart
    itself (keeper.json says its code) is asked to (DIR/RESTART); one from
    before that is stopped from outside: its jobs stopped on their hosts
    (their ssh would outlive it, and the new keeper would run them twice),
    then a new keeper, which puts them back on the queue."""
    pid = keeper_alive(q)
    if not pid:
        return ensure(q)
    try:
        info = json.load(open(q.p("keeper.json")))
    except (OSError, ValueError):
        info = {}
    cost = restart_cost(q)
    if info.get("code"):
        open(q.p("RESTART"), "w").close()
        q.event("restart asked")
        print("the keeper restarts at its next beat; %d running jobs run again" % cost["jobs"])
        return
    q.event("restart asked", how="stopped from outside: a keeper from before restarts")
    os.kill(pid, signal.SIGTERM)
    for _ in range(40):
        if not keeper_alive(q):
            break
        time.sleep(0.5)
    else:
        sys.exit("the keeper (pid %d) did not end" % pid)
    hosts = {h["name"]: h for h in q.config()["hosts"]}
    for j in q.jobs_in("running"):
        h = hosts.get(os.path.basename(os.path.dirname(j["_file"])))
        if h:
            stop_remote(h, j["id"])
    try:
        os.remove(q.p("keeper.pid"))
    except OSError:
        pass
    ensure(q)
    print("restarted: %d running jobs went back on the queue" % cost["jobs"])


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__)
    q = Queue(argv[0])
    cmd = argv[1]
    if cmd == "start":
        # A keeper that restarted itself is this process: its pid is ours.
        if keeper_alive(q) and keeper_alive(q) != os.getpid():
            sys.exit("a keeper is already running (pid %s)" % keeper_alive(q))
        for f in ("STOP_NOW", "SHUTDOWN"):
            try:
                os.remove(q.p(f))
            except OSError:
                pass
        keeper(q)
    elif cmd == "ensure":
        ensure(q)
    elif cmd == "plan-now":
        # The keeper plans when runs/ changes: every keeper, this one and
        # those from before the button.
        os.makedirs(q.p("runs"), exist_ok=True)
        os.utime(q.p("runs"))
        q.event("plan asked")
        print("the keeper asks the planner for work at its next beat")
    elif cmd == "restart":
        restart_keeper(q)
    elif cmd == "health":
        h = health(q)
        if "--json" in argv:
            print(json.dumps(h, indent=1))
        else:
            for c in h:
                print("%-4s %-10s %s" % ({"ok": "ok", "warn": "LOOK", "bad": "BAD"}[c["level"]], c["name"], c["text"]))
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
    elif cmd == "resolve":
        # The local line (promotions that self-modified, without push
        # permission or with auto mode off) pushed to main, from a machine
        # that can push.
        cfg = q.config()
        r = subprocess.run(["bash", "-c", "%s resolve" % cfg["planner"]], text=True)
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
