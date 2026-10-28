#!/usr/bin/env python3
"""What the fleet is training, as one JSON document (LAB-REPORT-BRAIN.md 109).

    python3 -I tools/plans/treadcore-view.py [--fleet grove] [--queue QDIR]

(Moved from treadcore's scripts/fleet-view.py, treadcore LR 113: orrery holds
the orchestration. TREADCORE= names the checkout, ~/code/treadcore by default.)

orrery's Training view draws this (`--jobs-cmd`). It reads, every time:

- the fleet's machines: the static list of the Copal fleet
  (~/.copal/fleets/NAME/nodes), and this Mac, the coordinator; each asked over
  ssh, in parallel and with a short timeout, for its cores, load, memory and
  GPU;
- what each is doing: the lanes of the newest recording run
  (tools/plans/treadcore-jobs.py), and the loop's state (build/fleet-train);
- the net being trained: the loop's best (build/fleet-train/best.bin, else
  nets/bc2.bin) read from its Net::save form, its weight matrices shrunk to
  at most 48 x 48 cells for drawing;
- how good it is: build/fleet-train/best.json (held-out agreement by head,
  the census), with bc1 and the official net beside it.
"""
import json
import os
import struct
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

TC = os.path.expanduser(os.environ.get("TREADCORE", "~/code/treadcore"))
FT = os.environ.get("FT_DIR", os.path.join(TC, "build", "fleet-train"))
FLEET = sys.argv[sys.argv.index("--fleet") + 1] if "--fleet" in sys.argv else "grove"
# --queue QDIR: an orrery fleet-queue (tools/fleet-queue.py) and its planner's
# summary, so the view shows the queue and what each machine runs in it.
QDIR = sys.argv[sys.argv.index("--queue") + 1] if "--queue" in sys.argv else ""
QUEUE_PY = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "fleet-queue.py")
STORE = os.environ.get("STORE", "user@192.168.8.181").split("@")[-1]
CENSUS = os.environ.get("CENSUS_HOST", "user@192.168.8.224").split("@")[-1]
HEADS = [("drive", 10), ("gun", 9), ("lead", 3), ("mortar", 9), ("drone", 5), ("squad", 7), ("signal", 4)]

PROBE = ("echo cores=$(nproc 2>/dev/null || sysctl -n hw.ncpu); "
         "echo load=$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg | awk '{print $2}'); "
         "free -m 2>/dev/null | awk '/Mem:/ {print \"mem_mb=\" $2; print \"avail_mb=\" $7}'; "
         "nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits 2>/dev/null | head -1 | sed 's/^/gpu=/'")


def read(path, default=""):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return default


def probe(addr):
    cmd = ["bash", "-c", PROBE] if addr == "local" else \
        ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=3", addr, PROBE]
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=6).stdout
    except (subprocess.TimeoutExpired, OSError):
        return {"up": False}
    d = {"up": bool(out.strip())}
    for line in out.splitlines():
        k, _, v = line.partition("=")
        if k in ("cores", "mem_mb", "avail_mb"):
            d[k] = int(v) if v.strip().isdigit() else None
        elif k == "load":
            try:
                d[k] = float(v)
            except ValueError:
                pass
        elif k == "gpu" and v.strip():
            p = [x.strip() for x in v.split(",")]
            if len(p) == 4:
                d["gpu"] = {"name": p[0].replace("NVIDIA GeForce ", ""), "busy_pct": int(p[1]),
                            "mem_mb": int(p[2]), "mem_total_mb": int(p[3])}
    return d


def machines(run, qd=None):
    nodes = []
    for line in read(os.path.expanduser(f"~/.copal/fleets/{FLEET}/nodes")).splitlines():
        line = line.split("#")[0].split()
        if len(line) >= 2:
            nodes.append((line[0], line[1]))
    nodes.append(("this Mac", "local"))
    with ThreadPoolExecutor(len(nodes)) as ex:
        found = list(ex.map(lambda n: probe(n[1] if n[1] == "local" else "user@" + n[1]), nodes))
    out = []
    for (name, addr), p in zip(nodes, found):
        roles = []
        if addr == STORE:
            roles.append("trains on the GPU")
        if addr == CENSUS:
            roles.append("runs the census")
        if addr == "local":
            roles.append("coordinates")
        lanes = {l["host"] for l in (run or {}).get("lanes", [])} | {l["host"] for l in (qd or {}).get("lanes", [])}
        if name in lanes or (addr == "local" and "mac" in lanes):
            roles.insert(0, "records matches")
        elif addr != "local":
            roles.append("not in this recording run")
        out.append({"name": name, "address": "" if addr == "local" else addr, "roles": roles, **p})
    return out


def ticker(n=40):
    """The newest coefficients the fleet changed, a line a variable, and the
    two Found Improvement leaderboards: by variable, and by machine."""
    out, by_var, by_host = [], {}, {}
    try:
        lines = open(os.path.join(QDIR, "plan", "ticker.jsonl")).read().splitlines()
    except OSError:
        lines = []
    for l in lines:
        try:
            e = json.loads(l)
        except ValueError:
            continue
        host = e.get("host") or "?"
        h = by_host.setdefault(host, [0, 0])
        h[0] += 1
        if e.get("improved"):
            h[1] += 1
        for var, v in (e.get("vars") or {}).items():
            value, offset = (v.get("value"), v.get("offset")) if isinstance(v, dict) else (v, None)
            if e.get("improved"):
                by_var[var] = by_var.get(var, 0) + 1
            out.append({"t": e.get("t", "")[11:19], "kind": e.get("kind", ""), "var": var, "value": value, "offset": offset,
                        "d": e.get("d"), "se": e.get("se"), "host": host, "improved": bool(e.get("improved"))})
    return {"ticker": out[-n:],
            "lead_vars": sorted(by_var.items(), key=lambda kv: -kv[1])[:10],
            "lead_hosts": sorted(([k, v[0], v[1]] for k, v in by_host.items()), key=lambda r: (-r[2], -r[1]))}


def runs():
    """The runs a person asked for, and where each is (the planner's `runs`)."""
    try:
        cfg = json.load(open(os.path.join(QDIR, "config.json")))
        out = subprocess.run(["bash", "-c", cfg["planner"] + " runs"], capture_output=True, text=True, timeout=30).stdout
        return json.loads(out)
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
        return []


def queue():
    if not QDIR or not os.path.exists(os.path.join(QDIR, "config.json")):
        return None
    try:
        out = subprocess.run([sys.executable, QUEUE_PY, QDIR, "status", "--json"], capture_output=True, text=True, timeout=20).stdout
        q = json.loads(out)
    except (subprocess.TimeoutExpired, ValueError):
        return None
    q["name"] = os.path.basename(os.path.abspath(QDIR))
    q.update(ticker())
    q["runs"] = runs()
    try:
        q["push"] = json.load(open(os.path.join(QDIR, "plan", "push.json")))
    except (OSError, ValueError):
        q["push"] = None
    s = read(os.path.join(QDIR, "plan", "summary.json"))
    if s:
        try:
            q["summary"] = json.loads(s)
        except ValueError:
            pass
    return q


def newest_run():
    d = os.path.join(TC, "build", "fleet")
    try:
        runs = [os.path.join(d, r) for r in os.listdir(d) if os.path.isdir(os.path.join(d, r, "queue"))]
    except OSError:
        return None
    runs = [r for r in runs if not os.path.basename(r).startswith("rehearse")]
    if not runs:
        return None
    r = max(runs, key=lambda r: os.path.getmtime(os.path.join(r, "manifest.tsv")) if os.path.exists(os.path.join(r, "manifest.tsv")) else 0)
    try:
        out = subprocess.run([sys.executable, "-I", os.path.join(os.path.dirname(os.path.abspath(__file__)), "treadcore-jobs.py"), r],
                             capture_output=True, text=True, timeout=10).stdout
        return json.loads(out)["runs"][0]
    except (subprocess.TimeoutExpired, ValueError, KeyError, IndexError):
        return None


def loop():
    rnd = read(os.path.join(FT, "round"), "0")
    stages = []
    events = []
    try:
        with open(os.path.join(FT, "events.jsonl")) as f:
            events = [json.loads(l) for l in f if l.strip()]
    except OSError:
        pass
    now = {}
    for e in events:
        if str(e.get("round")) == rnd and e.get("stage") not in ("round", "loop"):
            now[e["stage"]] = e["event"]
    for s in ("record", "train", "judge"):
        stages.append({"stage": s, "state": now.get(s, "")})
    return {"round": int(rnd) if rnd.isdigit() else 0, "stages": stages, "best": read(os.path.join(FT, "best")),
            "stopping": os.path.exists(os.path.join(FT, "STOP")),
            # A round is running when its last round-event is "started".
            "running": next((e.get("event") == "started" for e in reversed(events) if e.get("stage") == "round"), False)}


def shrink(w, rows, cols, cap=48):
    """A rows x cols matrix (row-major) as at most cap x cap cells, each the
    mean of the weights it covers."""
    r2, c2 = min(rows, cap), min(cols, cap)
    out = []
    for i in range(r2):
        r0, r1 = i * rows // r2, max(i * rows // r2 + 1, (i + 1) * rows // r2)
        row = []
        for j in range(c2):
            c0, c1 = j * cols // c2, max(j * cols // c2 + 1, (j + 1) * cols // c2)
            vals = [w[a * cols + b] for a in range(r0, r1) for b in range(c0, c1)]
            row.append(round(sum(vals) / len(vals), 4))
        out.append(row)
    return out


def net():
    path = os.path.join(FT, "best.bin")
    name = read(os.path.join(FT, "best"), "").split(" ")[0] or "bc2"
    if not os.path.exists(path):
        path = os.path.join(TC, "nets", "bc2.bin")
    try:
        b = open(path, "rb").read()
    except OSError:
        return None
    if b[:8] != b"TCNET1\0\0":
        return None
    k = struct.unpack_from("<I", b, 8)[0]
    sizes = list(struct.unpack_from(f"<{k}I", b, 12))
    off = 12 + 4 * k
    layers = []
    for a, o in zip(sizes, sizes[1:]):
        w = struct.unpack_from(f"<{a * o}f", b, off)
        off += 4 * a * o
        off += 4 * o  # biases, not drawn
        m = max(abs(x) for x in w) or 1.0
        # Whole, not shrunk: orrery draws a matrix large when it is clicked.
        layers.append({"inputs": a, "outputs": o, "max_abs": round(m, 4), "cells": shrink(w, o, a, cap=1000)})
    # What the 143 inputs are, by group (senses.rs: SECTORS * CHANNELS, BODY,
    # NAV, CONTACTS, HINTS, SHELL, AIR), and the 47 outputs by head.
    groups, at = [], 0
    for g, n in [("compass", 8 * 11), ("body", 12), ("navigation", 6), ("contacts", 18),
                 ("hints", 9), ("shells", 5), ("air", 5)]:
        groups.append({"name": g, "from": at, "to": at + n - 1})
        at += n
    heads, at = [], 0
    for h, n in HEADS:
        heads.append({"name": h, "from": at, "to": at + n - 1})
        at += n
    return {"name": name, "file": os.path.relpath(path, TC), "sizes": sizes,
            "input_groups": groups if sizes and sizes[0] == 143 else [],
            "output_groups": heads if sizes and sizes[-1] == 47 else [],
            "parameters": sum(a * o + o for a, o in zip(sizes, sizes[1:])),
            "heads": [{"name": n, "choices": c} for n, c in HEADS], "layers": layers}


def quality():
    q = read(os.path.join(FT, "best.json"))
    if q:
        try:
            return json.loads(q)
        except ValueError:
            pass
    # Until the loop has promoted a net: bc2, as LR 108 read it.
    return {
        "net": "bc2",
        "held_out": {"drive": 66.6, "gun": 62.7, "mortar": 89.9, "drone": 96.8, "lead": 99.8},
        "bc1": {"drive": 65.5, "gun": 64.2, "mortar": 89.3, "drone": 96.7, "lead": 99.8},
        "commonest": {"drive": 36.5, "gun": 37.9, "mortar": 87.7, "drone": 94.0, "lead": 99.8},
        "census": {"best": -19.76, "best_se": 0.39, "official": -20.33, "official_se": 0.27,
                   "vs_official": 0.57, "vs_official_se": 0.37},
        "source": "LAB-REPORT-BRAIN.md section 108",
    }


if __name__ == "__main__":
    run = newest_run()
    qd = queue()
    doc = {"fleet": FLEET, "queue": qd, "project": "treadcore", "at": time.strftime("%H:%M:%S"),
           "machines": machines(run, qd), "run": run, "loop": loop(), "net": net(), "quality": quality()}
    print(json.dumps(doc))
