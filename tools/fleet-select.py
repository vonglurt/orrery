#!/usr/bin/env python3
"""Select machines by what they are NOW, rather than by a name someone wrote down.

    fleet-select.py os=ubuntu
    fleet-select.py os=alpine idle
    fleet-select.py 'version>=24.04' has:nvidia-smi
    fleet-select.py 'treadcore>=2026-10-29' 'mem_gb>=16'
    fleet-select.py 'treadcore!=37b6b65'         every machine not on that commit
    fleet-select.py os=ubuntu or os=alpine idle --table
    fleet-select.py idle --run uptime            the same verbs the Lab's Run sends

Words are ANDed; `or` starts another group. A word is:

    KEY=VALUE KEY!=VALUE       the fact, compared as text, or a commit by its prefix
    KEY>=V KEY<=V KEY>V KEY<V  dotted versions and dates by their numbers
    KEY                        the fact is present and not empty (gpu, cuda)
    idle                       under a quarter of its processors busy, nobody logged in
    has:NAME                   a program or a package, asked of the machine

THE ANSWER IS ASKED OF THE MACHINE, through the same forced command as every
button: `facts` and `has` over the fleet's certificate. A machine that does not
answer is said to, on stderr, and selected by nothing: a machine nobody could
ask is not a machine that matched. No inventory is written down, because the
one thing an inventory reliably says is what was true when it was written.
"""
import concurrent.futures as cf
import os
import re
import subprocess
import sys

FLEET = os.environ.get("COPAL_FLEET", "grove")
FDIR = os.environ.get("COPAL_FLEET_DIR") or os.path.expanduser("~/.copal/fleets/%s" % FLEET)
# COPAL_SSH_OPTS: the check's containers answer on their own ports.
SSH = ["ssh", "-n", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6", "-i", FDIR + "/operator"] + os.environ.get("COPAL_SSH_OPTS", "").split()
OPS = ("!=", ">=", "<=", "=", ">", "<")


def machines():
    out = []
    with open(FDIR + "/nodes") as f:
        for line in f:
            w = line.split("#")[0].split()
            if len(w) >= 2:
                out.append((w[0], w[1]))
    return out


def ask(addr, verb):
    r = subprocess.run(SSH + ["copal-fleet@%s" % addr, verb], capture_output=True, text=True, timeout=40)
    return r.returncode, r.stdout.strip(), r.stderr.strip()


def facts(addr):
    rc, out, err = ask(addr, "facts")
    if rc != 0:
        raise RuntimeError(err.splitlines()[-1] if err else "no answer")
    f = dict(l.split("=", 1) for l in out.splitlines() if "=" in l)
    # A checkout's fact is "37b6b65 2026-10-29": its commit and its date, and a
    # selection means whichever of the two it was given.
    for k in ("treadcore", "crucible", "orrery", "copal-tm"):
        if " " in f.get(k, ""):
            f[k + ".commit"], f[k + ".date"] = f[k].split(" ", 1)
    return f


def numbers(s):
    """2026-10-29 and 24.04 and 3.24.3, as what sorts them."""
    return [int(n) for n in re.findall(r"\d+", s)]


def is_idle(f):
    """A quarter of the processors: the fleet's lanes run at nice 19, so a busy
    machine is one with work on it whether or not it would give the work up."""
    return float(f.get("load", "99")) < int(f.get("cpus", "1")) / 4 and f.get("users", "1") == "0"


def word_matches(word, f, addr):
    if word == "idle":
        return is_idle(f)
    if word.startswith("has:"):
        return ask(addr, "has " + word[4:])[0] == 0
    for op in OPS:
        if op in word:
            k, want = word.split(op, 1)
            break
    else:
        return bool(f.get(word))
    if k not in f:
        return False
    have = f[k]
    if k + ".commit" in f:
        # A date is compared with the date, anything else with the commit.
        have = f[k + ".date"] if re.fullmatch(r"\d{4}-\d{2}-\d{2}", want) else f[k + ".commit"]
    if op in ("=", "!="):
        same = have == want or (k + ".commit" in f and have.startswith(want))
        return same if op == "=" else not same
    a, b = numbers(have), numbers(want)
    return {">=": a >= b, "<=": a <= b, ">": a > b, "<": a < b}[op]


def parse(argv):
    groups, run, table = [[]], None, False
    i = 0
    while i < len(argv):
        w = argv[i]
        if w == "--run":
            i += 1
            run = argv[i]
        elif w == "--table":
            table = True
        elif w == "or":
            groups.append([])
        else:
            groups[-1].append(w)
        i += 1
    return groups, run, table


def main(argv):
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    groups, run, table = parse(argv)
    nodes = machines()
    with cf.ThreadPoolExecutor(len(nodes)) as ex:
        got = list(ex.map(lambda n: _facts(n), nodes))
    chosen = []
    for (name, addr), f in zip(nodes, got):
        if isinstance(f, Exception):
            print("%s: not asked: %s" % (name, f), file=sys.stderr)
            continue
        if any(all(word_matches(w, f, addr) for w in g) for g in groups):
            chosen.append((name, addr, f))
    if table:
        for name, _, f in chosen:
            print("%-14s %-7s %-8s load %-6s of %-3s %-6s treadcore %s" % (
                name, f.get("os", ""), f.get("version", ""), f.get("load", ""), f.get("cpus", ""),
                "idle" if is_idle(f) else "busy", f.get("treadcore", "-")))
    elif not run:
        for name, _, _ in chosen:
            print(name)
    if run:
        for name, addr, _ in chosen:
            rc, out, err = ask(addr, run)
            print("%-14s %s %s" % (name, "ok  " if rc == 0 else "FAIL", (out or err).splitlines()[0] if (out or err) else ""))
    return 0 if chosen else 1


def _facts(n):
    try:
        return facts(n[1])
    except Exception as e:
        return e


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
