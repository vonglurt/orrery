#!/usr/bin/env python3
"""fleet-versions.py [--write FILE] -- what every machine runs: each project's commit and
date, a row a machine, and the line main is on. For a mass deployment the question is
which machine is on what, so it is asked of the machines rather than remembered.

Over ordinary ssh, as the user; reads only. A checkout a machine does not have is a dash,
which is not a fault: not every machine runs every project. Exit 1 when a machine that
has a project is not on its main's line.
"""
import concurrent.futures as cf
import os
import subprocess
import sys

PROJECTS = ["treadcore", "orrery", "crucible", "copal-tm"]
NODES = os.path.expanduser("~/.copal/fleets/grove/nodes")
SSH = ["ssh", "-n", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6"]
SCRIPT = "for p in %s; do d=$HOME/code/$p; if [ -d $d/.git ]; then echo \"$p $(git -C $d log -1 --format=%%h) $(git -C $d log -1 --format=%%cs) $(git -C $d status --short | grep -vc '^??')\"; fi; done" % " ".join(PROJECTS)


def machines():
    out = []
    for line in open(NODES):
        w = line.split("#")[0].split()
        if len(w) >= 2:
            out.append((w[0], w[1]))
    return out


def ask(n):
    r = subprocess.run(SSH + ["user@" + n[1], SCRIPT], capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        return n[0], None
    return n[0], {w[0]: w[1:] for w in (l.split() for l in r.stdout.splitlines()) if len(w) == 4}


def here():
    out = {}
    for p in PROJECTS:
        d = os.path.expanduser("~/code/" + p)
        if os.path.isdir(d + "/.git"):
            g = lambda *a: subprocess.run(["git", "-C", d] + list(a), capture_output=True, text=True).stdout.strip()
            dirty = sum(1 for l in g("status", "--short").splitlines() if not l.startswith("??"))
            out[p] = [g("log", "-1", "--format=%h"), g("log", "-1", "--format=%cs"), str(dirty)]
    return out


def main(argv):
    with cf.ThreadPoolExecutor(12) as ex:
        rows = list(ex.map(ask, machines()))
    rows.append(("this Mac", here()))
    head = here()
    lines = ["%-14s %s" % ("machine", " ".join("%-20s" % p for p in PROJECTS))]
    bad = 0
    for name, r in rows:
        if r is None:
            lines.append("%-14s no answer" % name)
            bad += 1
            continue
        cells = []
        for p in PROJECTS:
            v = r.get(p)
            if not v:
                cells.append("%-20s" % "-")
                continue
            off = p in head and v[0] != head[p][0]
            bad += off
            cells.append("%-20s" % (v[0] + " " + v[1][5:] + ("*" if off else "") + ("+" if v[2] != "0" else "")))
        lines.append("%-14s %s" % (name, " ".join(cells)))
    lines.append("* not this Mac's commit   + changes of its own")
    text = "\n".join(lines)
    print(text)
    if "--write" in argv:
        open(argv[argv.index("--write") + 1], "w").write(text + "\n")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
