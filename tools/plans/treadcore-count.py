#!/usr/bin/env python3
"""How much a fleet queue has played and trained (LAB-REPORT-BRAIN.md 111).

    python3 -I tools/plans/treadcore-count.py ~/code/treadcore/build/queue/NAME

It reads what the queue has fetched back, never a host:

- every census log (results/JOB/**/g-*.log): a game is a line
  `seed S on MAP: trial team T, over ...`; the seat-ticks, thinks and moves
  are the job's own meter line (`brain_census: trial ... meter:`);
- every net round's trainer log (results/train-NNN/**/train.log): its
  training samples times its epochs;
- events.jsonl: the jobs done by host and kind, and the seconds each host
  was busy.

A job still running is not counted until its results are fetched.
"""
import collections
import glob
import json
import os
import re
import sys

q = sys.argv[1]
GAME = re.compile(r"^seed \S+ on \S+: trial team \d+, over")
METER = re.compile(r"^brain_census: trial .*meter: (\d+) seat-ticks, (\d+) thinks, (\d+) moves")
KIND = {"w": "weight trial", "h": "held-out read", "j": "net judge"}

by = collections.defaultdict(lambda: [0, 0, 0, 0, 0])  # kind -> logs, games, seat-ticks, thinks, moves
for f in glob.glob(os.path.join(q, "results", "*", "**", "g-*.log"), recursive=True):
    job = os.path.relpath(f, os.path.join(q, "results")).split(os.sep)[0]
    b = by[KIND.get(job[0], job)]
    b[0] += 1
    for line in open(f, errors="replace"):
        if GAME.match(line):
            b[1] += 1
        m = METER.match(line)
        if m:
            b[2] += int(m[1]); b[3] += int(m[2]); b[4] += int(m[3])
total = [sum(v[i] for v in by.values()) for i in range(5)]
for k, v in sorted(by.items()) + [("all", total)]:
    print("%-14s logs %5d  games %7d  seat-ticks %14d  thinks %13d  moves %12d" % (k, *v))
print("game time %.0f hours (900 s a game)" % (total[1] * 900 / 3600))

for f in sorted(glob.glob(os.path.join(q, "results", "train-*", "**", "train.log"), recursive=True)):
    text = open(f, errors="replace").read()
    n = re.search(r"training on (\d+)", text)
    e = re.findall(r"^bc: epoch +(\d+):", text, re.M)
    if n and e:
        print("%s  %d samples x %d epochs = %d sample-steps" % (
            os.path.relpath(f, q).split(os.sep)[1], int(n[1]), int(e[-1]), int(n[1]) * int(e[-1])))

ev = [json.loads(line) for line in open(os.path.join(q, "events.jsonl"))]
done = [e for e in ev if e.get("event") == "done"]
if done:
    print("from %s to %s, %d jobs done" % (ev[0]["t"], done[-1]["t"], len(done)))
jobs = collections.Counter((e["host"], e["kind"]) for e in done)
busy = collections.Counter()
for e in done:
    busy[e["host"]] += e.get("seconds", 0)
for host in sorted(busy):
    kinds = ", ".join("%s %d" % (k, n) for (h, k), n in sorted(jobs.items()) if h == host)
    print("  %-10s busy %4.1f h  (%s)" % (host, busy[host] / 3600, kinds))
