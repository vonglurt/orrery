#!/usr/bin/env python3
"""A fleet recording run as one JSON document (LAB-REPORT-BRAIN.md section 109).

    python3 -I tools/plans/treadcore-jobs.py ~/code/treadcore/build/fleet/RUN [...]

It reads only the run's files (scripts/fleet-record.sh writes them): the
queue, each lane's heartbeat, the manifest, the hosts and the store. For
each host it says what it is doing now, and the command pipe that does it,
as LAB-REPORT-FLEET.md III spells them. orrery's museum interface draws this
document in its Jobs view (`--jobs-cmd`), and fleet-train's `status --json`
carries it for the round in hand.
"""
import json
import os
import sys
import time


def lines(path):
    try:
        with open(path) as f:
            return [l.rstrip("\n") for l in f if l.strip() and not l.startswith("#")]
    except FileNotFoundError:
        return []


def files(path):
    try:
        return sorted(f for f in os.listdir(path) if not f.startswith("."))
    except FileNotFoundError:
        return []


def pipe(state, host, target, store, run, job, threads, nice):
    """The command a lane runs in this state, as a person would type it."""
    short = target if target == "-" else target.split("@")[-1]
    s = store.split("@")[-1]
    shard = f"~/fleet/{run}/out/{job}.bin"
    if state == "recording":
        cmd = (f"RECORD={shard} SEED0={job} THREADS={threads} nice -n {nice} "
               f"census-bin --ignored learn::harness::train | tail -4")
        return cmd if target == "-" else f"ssh {short} \"{cmd}\""
    if state == "sending":
        if target == store:
            return f"ssh {s} \"mv {shard} ~/fleet/{run}/shards/{job}.bin.part && sha256sum\""
        if target == "-":
            return f"cat {shard} | ssh {s} \"cat > ~/fleet/{run}/shards/{job}.bin.part\""
        return f"ssh {short} \"cat {shard}\" | ssh {s} \"cat > ~/fleet/{run}/shards/{job}.bin.part\""
    return ""


def run_doc(d):
    run = os.path.basename(os.path.normpath(d))
    store = (lines(os.path.join(d, "store")) or [""])[0]
    hosts = [l.split() for l in lines(os.path.join(d, "hosts"))]
    manifest = [l.split("\t") for l in lines(os.path.join(d, "manifest.tsv"))]
    queued = len(files(os.path.join(d, "queue")))
    failed = len(files(os.path.join(d, "failed")))
    now = time.time()
    lanes, running, done = [], 0, 0
    for h in hosts:
        if len(h) < 4:
            continue
        name, target, threads, nice = h[:4]
        claimed = files(os.path.join(d, "claimed", name))
        dn = len(files(os.path.join(d, "done", name)))
        running += len(claimed)
        done += dn
        # "<time> <job> recording|sending", or "<time> finished" (or a bare
        # time, from fleet-record.sh before 990b5d8).
        beat = (lines(os.path.join(d, "lanes", name + ".beat")) or [""])[0].split()
        at = float(beat[0]) if beat and beat[0].isdigit() else None
        if at is None:
            state, job = "waiting to start", ""
        elif len(beat) >= 3:
            state, job = beat[2], beat[1]
        else:
            state, job = (beat[1] if len(beat) == 2 else "finished"), ""
        mine = [m for m in manifest if len(m) >= 9 and m[1] == name]
        seeds = sum(int(m[3]) for m in mine)
        secs = sum(int(m[8]) for m in mine)
        lanes.append({
            "host": name, "target": target, "threads": int(threads), "nice": int(nice),
            "jobs": dn, "seeds": seeds, "seeds_per_s": round(seeds / secs, 3) if secs else 0,
            "state": state, "job": job, "age_s": int(now - at) if at else None,
            "pipe": pipe(state, name, target, store, run, job, threads, nice),
        })
    total = queued + running + done + failed
    return {
        "run": run, "dir": os.path.abspath(d), "store": store,
        "queued": queued, "running": running, "done": done, "failed": failed, "total": total,
        "seeds": sum(int(m[3]) for m in manifest if len(m) >= 9),
        "samples": sum(int(m[4]) for m in manifest if len(m) >= 9),
        "bytes": sum(int(m[5]) for m in manifest if len(m) >= 9),
        "ended": queued == 0 and running == 0 and total > 0,
        "lanes": lanes,
    }


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    runs = [run_doc(d) for d in sys.argv[1:]]
    print(json.dumps({"runs": runs}, indent=1))
