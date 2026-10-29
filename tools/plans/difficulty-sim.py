#!/usr/bin/env python3
"""difficulty-sim.py: what each ease buys, by simulation.

    difficulty-sim.py [QUEUE] [--mu M --tau T --rate SEEDS_AN_HOUR] [--tail F --tail-mu G]

The trials' true gains are drawn from N(mu, tau), a seat; each is read as the
planner reads it (treadcore.py's own SEQ and HELD_Z, every bar lowered by the
ease), a seed swinging SD. Given a queue, mu, tau and the rate are measured
from it: tau is what the first reads' spread holds past their own noise, and
the rate is the seeds its weight jobs read in the last day. For each ease:
how often a promotion comes, what each truly gains, how many are truly
worse, and the gain a day. --tail F makes a share F of the trials truly
+G (--tail-mu, 0.6): the few real finds a normal of the rest leaves out.
The ease to aim at is the one with the most gain
a day, not the one with the most promotions (29 October: one in 20 minutes
was asked for, and there the brain grows worse each day).
"""
import datetime
import glob
import importlib.util
import json
import math
import os
import random
import re
import statistics
import sys

SD = 4.9          # a seed's swing, a seat (the census's own, 29 October)
HERE = os.path.dirname(os.path.abspath(__file__))


def planner():
    s = importlib.util.spec_from_file_location("treadcore_plan", os.path.join(HERE, "treadcore.py"))
    m = importlib.util.module_from_spec(s)
    s.loader.exec_module(m)
    return m


def measure(m, q):
    """(mu, tau, seeds an hour) from a queue."""
    p = m.Plan(q)
    # Every trial's own first census, not the trials read only once: those
    # are the ones dropped, the worst (the first measure gave N(-0.54, 0.22)
    # so, against N(-0.32, 0.44) from every trial's first read).
    seq, first = p.sequential(), []
    for d in p.census_dirs():
        jid = d.rstrip("/").split(os.sep)[-2]
        r = p.pool([d]) if jid in seq and re.match(r"w\d+$", jid) else None
        if r:
            first.append(r["every"][:2])
    ds = [d for d, _ in first]
    tau = math.sqrt(max(0.0, statistics.pvariance(ds) - statistics.mean(se * se for _, se in first)))
    since = datetime.datetime.now() - datetime.timedelta(days=1)
    seeds = 0
    for f in glob.glob(os.path.join(q, "done", "*.json")):
        j = json.load(open(f))
        n = re.search(r"SEEDS=(\d+)", j.get("cmd", ""))
        if j.get("kind") == "weights" and n and j.get("rc") == 0 and datetime.datetime.fromisoformat(j.get("ended", "1970-01-01")) > since:
            seeds += int(n.group(1))
    return statistics.mean(ds), tau, seeds / 24.0


def run(seq, held_z, base, held, ease, mu, tau, rate, n, tail=0.0, tail_mu=0.6):
    seeds, got = 0, []
    for _ in range(n):
        g = random.gauss(tail_mu, 0.1) if random.random() < tail else random.gauss(mu, tau)
        s, tot, passed = 0, 0.0, False
        for m_, keep, ok in seq:
            k = int(round(m_ * base))
            tot += random.gauss(g * (k - s), SD * math.sqrt(k - s))
            s = k
            z = (tot / s) / (SD / math.sqrt(s))
            if ok is not None and z >= ok - ease:
                passed = True
                break
            if keep is None or z < keep - ease:
                break
        seeds += s
        if passed:
            seeds += held
            if random.gauss(g, SD / math.sqrt(held)) / (SD / math.sqrt(held)) >= held_z - ease:
                got.append(g)
    hours = seeds / rate
    return (len(got) / hours, statistics.mean(got) if got else 0.0,
            sum(x < 0 for x in got) / len(got) if got else 0.0, 24 * sum(got) / hours)


def main():
    a = sys.argv[1:]
    opt = {k: float(a[a.index(k) + 1]) for k in ("--mu", "--tau", "--rate", "--tail", "--tail-mu") if k in a}
    q = a[0] if a and not a[0].startswith("--") else None
    m = planner()
    if q:
        mu, tau, rate = measure(m, q)
    else:
        mu, tau, rate = -0.45, 0.3, 5400.0
    mu, tau, rate = opt.get("--mu", mu), opt.get("--tau", tau), opt.get("--rate", rate)
    tail, tail_mu = opt.get("--tail", 0.0), opt.get("--tail-mu", 0.6)
    print("trials N(%+.2f, %.2f) a seat%s, a seed's swing %.1f, %.0f seeds an hour" % (
        mu, tau, ", %.1f%% of them +%.2f" % (100 * tail, tail_mu) if tail else "", SD, rate))
    print("ease   one every   true gain each   truly worse   gain a day")
    random.seed(1)
    for ease in (0, 0.5, 1, 1.5, 2, 2.5, 3, 4):
        r, mean, worse, day = run(m.Plan.SEQ, m.Plan.HELD_Z, 55, 800, ease, mu, tau, rate, 20000, tail, tail_mu)
        every = "%6.0f min" % (60 / r) if r else "     never"
        print("%4.1f  %s       %+.2f          %3.0f%%        %+.2f%s" % (
            ease, every, mean, 100 * worse, day, "   (the floor)" if ease == m.Plan.EASE_MAX else ""))


if __name__ == "__main__":
    main()
