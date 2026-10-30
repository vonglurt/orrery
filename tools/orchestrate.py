#!/usr/bin/env python3
"""orchestrate.py QUEUE COMMAND ... -- what the Fleet screen reads and drives.

    status              the CA, the operator's certificate, each machine, each
                        button, as JSON; reads the last survey, no ssh
    survey              ask every machine again: its facts through its forced
                        command, its host certificate as it presents it
    select WORD ...     the machines that match, as JSON (fleet-select.py's words)
    send A,B VERB ...   one verb to each, through its forced command, as JSON
    text A,B VERB -- T  clip or save NAME: text T on stdin, or this Mac's clipboard
                        when T is absent
    chat                Fleet Chat: sync every machine with the room, the room as JSON
    say -- TEXT         a line in the room, from this console
    shot A,B            each machine's screen, saved in QUEUE/shots and opened
    login               an operator certificate, from `copal fleet login`

THE CONSOLE NEVER MINTS (docs/lockdown.md, section 5). Renewing is a person's press,
and what it runs is `copal fleet login`; the CA's private key is read by that
program and by nothing here. Installing is a person's password in a Sessions tile:
root is never run from here (machines.py stage, as every recipe).

NOTHING HERE DECIDES WHAT A MACHINE MAY DO. `send` checks a verb against the list
below so that a typing slip fails fast, and the list is a courtesy: the boundary is
the forced command on the machine, which refuses whatever this sends it.
"""
import concurrent.futures as cf
import fcntl
import importlib.util
import json
import os
import re
import subprocess
import sys
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("fleet_select", os.path.join(TOOLS, "fleet-select.py"))
sel = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sel)

# Stage the listener again when a machine's version is older than this file's.
with open(os.path.join(TOOLS, "fleet-listener.sh")) as _f:
    CURRENT = re.search(r"^VERSION=(\d+)", _f.read(), re.M).group(1)

VERBS = {"id", "uptime", "facts", "has", "message", "last-message", "scene", "power", "snapshot", "log"}


def run(argv, timeout=20, stdin=None):
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout, input=stdin)
    except (subprocess.TimeoutExpired, OSError) as e:
        return subprocess.CompletedProcess(argv, 1, "", str(e))


def fingerprint(path):
    r = run(["ssh-keygen", "-l", "-f", path])
    return r.stdout.split()[1] if r.returncode == 0 and r.stdout else ""


def cert_info(text_path):
    """(principals, seconds left, signing CA) from ssh-keygen -L, or Nones."""
    r = run(["ssh-keygen", "-L", "-f", text_path])
    if r.returncode != 0:
        return [], None, ""
    principals, left, ca = [], None, ""
    lines = r.stdout.splitlines()
    for i, l in enumerate(lines):
        s = l.strip()
        if s.startswith("Signing CA:"):
            m = re.search(r"(SHA256:\S+)", s)
            ca = m.group(1) if m else ""
        elif s.startswith("Valid:"):
            m = re.search(r"to (\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)", s)
            if m:
                left = int(time.mktime(time.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S")) - time.time())
            elif "forever" in s:
                left = 10 ** 9
        elif s.startswith("Principals:"):
            for p in lines[i + 1:]:
                if not p.startswith(" " * 16):
                    break
                principals.append(p.strip())
    return principals, left, ca


def presented(addr, ca_fp):
    """The certificate a machine shows anyone who asks: (days left, signed by ours)."""
    r = run(["ssh-keyscan", "-c", "-T", "5", "-t", "ed25519", addr], timeout=12)
    for line in r.stdout.splitlines():
        if "cert-v01" in line and not line.startswith("#"):
            p = "/tmp/orchestrate-%d-%s.pub" % (os.getpid(), re.sub(r"\W", "_", addr))
            with open(p, "w") as f:
                # -c prints no host in front of the key, unlike a plain scan.
                f.write(line[line.index("ssh-ed25519-cert"):] + "\n")
            try:
                _, left, ca = cert_info(p)
            finally:
                os.remove(p)
            return (left // 86400 if left is not None else -1), ca == ca_fp
    return None, False


class Fleet:
    def __init__(self, queue):
        self.q = queue
        self.cache = os.path.join(queue, "orchestrate.json")
        self.ca_pub = os.path.expanduser("~/.copal/ca/%s_ca.pub" % sel.FLEET)

    def known(self):
        try:
            with open(os.path.join(self.q, "machines.json")) as f:
                d = json.load(f)
            return d.get("machines", d) if isinstance(d, dict) else {}
        except (OSError, ValueError):
            return {}

    def survey(self):
        ca_fp = fingerprint(self.ca_pub)
        known = self.known()

        def one(n):
            name, addr = n
            m = {"name": name, "addr": addr, "listener": "no"}
            f = known.get(name, {}).get("facts") or {}
            m.update(os=f.get("os_id", ""), version=f.get("os_version", ""), cpus=f.get("cpus", ""),
                     treadcore=f.get("treadcore", ""))
            days, ours = presented(addr, ca_fp)
            m.update(cert_days=-1 if days is None else days, cert_ours=ours)
            try:
                got = sel.facts(addr)
                old = got.get("listener_version", "") != CURRENT
                m.update(listener="old" if old else "yes", facts=got, desktop=got.get("desktop", ""), os=got.get("os", ""), version=got.get("version", ""),
                         cpus=got.get("cpus", ""), load=got.get("load", ""), users=got.get("users", ""),
                         idle=sel.is_idle(got), treadcore=got.get("treadcore", m["treadcore"]),
                         gpu=got.get("gpu", ""), scene=got.get("scene", ""),
                         last_message=got.get("last_message", ""))
            except Exception as e:
                # Refused is the ordinary state before the install; anything else is news.
                why = str(e)
                if "denied" not in why:
                    m["listener"] = "no answer"
                    m["error"] = why[-120:]
            return m

        nodes = sel.machines()
        with cf.ThreadPoolExecutor(len(nodes)) as ex:
            ms = list(ex.map(one, nodes))
        tmp = self.cache + ".tmp"
        with open(tmp, "w") as f:
            json.dump({"at": time.strftime("%H:%M:%S"), "machines": ms}, f)
        os.rename(tmp, self.cache)
        return ms

    def status(self):
        try:
            with open(self.cache) as f:
                c = json.load(f)
        except (OSError, ValueError):
            c = {"at": "", "machines": [{"name": n, "addr": a, "listener": "not asked yet"} for n, a in sel.machines()]}
        ms = c["machines"]
        principals, left, _ = cert_info(sel.FDIR + "/operator-cert.pub")
        n = len(ms)
        up = sum(1 for m in ms if m.get("listener") == "yes")
        answering = sum(1 for m in ms if m.get("listener") in ("yes", "old"))
        for m in ms:
            m.pop("facts", None)

        def b(name, does, ready, why):
            return {"name": name, "does": does, "ready": ready, "total": n, "why": why}

        wanting = "" if answering == n else "waits for the listener on %d" % (n - answering)
        updating = "" if up == n else "waits for Install or Update on %d" % (n - up)
        return {
            "fleet": sel.FLEET, "surveyed": c.get("at", ""),
            "ca": {"path": self.ca_pub, "fingerprint": fingerprint(self.ca_pub), "ok": os.path.exists(self.ca_pub)},
            "operator": {"principals": principals, "left_min": -1 if left is None else left // 60},
            "machines": ms,
            "buttons": [
                b("Message", "login, a window, a notification, the terminals", up, updating),
                b("Clip", "~/clipboard.txt, and the desktop's clipboard if any", up, updating),
                b("Chat", "a window, or fleet-chat on a terminal", up, updating),
                b("Shot", "says hello you are being observed, then the picture or text", up, updating),
                b("Save", "text to a named file in ~/fleet-inbox", up, updating),
                b("Run", "a verb from the forced command's list", answering, wanting),
                b("Scene", "records a label on the machine; applies nothing", answering, wanting),
                b("Power", "off and reboot; on needs a switched port", answering, wanting),
                b("Terminal", "the user's shell, on a human's certificate", answering, wanting),
                b("Exchange", "files both ways over SFTP, one machine", answering, wanting),
                b("Send", "files one way over SFTP, many machines", answering, wanting),
                b("Control", "the machine's screen over RDP", 0, "no machine serves RDP yet"),
                b("Snapshot", "restores a snapshot", 0, "nothing takes snapshots here: it refuses"),
            ],
        }

    def login(self):
        r = run(["copal", "fleet", "login", "--fleet", sel.FLEET], timeout=30)
        if r.returncode != 0:
            raise SystemExit((r.stderr or "login failed").strip().splitlines()[-1])
        p, left, _ = cert_info(sel.FDIR + "/operator-cert.pub")
        return {"said": "operator certificate renewed: %s, %d h" % (", ".join(p), (left or 0) // 3600)}

    def select(self, words):
        groups, _, _ = sel.parse(words)
        nodes = sel.machines()
        with cf.ThreadPoolExecutor(len(nodes)) as ex:
            got = list(ex.map(sel._facts, nodes))
        chosen, unasked = [], []
        for (name, addr), f in zip(nodes, got):
            if isinstance(f, Exception):
                unasked.append(name)
            elif any(all(sel.word_matches(w, f, addr) for w in g) for g in groups):
                chosen.append(name)
        return {"chosen": chosen, "unasked": unasked}

    def text(self, names, verb, data):
        word = verb.split()[0]
        if word not in ("clip", "save"):
            raise SystemExit("no text verb called %s" % word)
        if not data:
            raise SystemExit("nothing to send: type it, or copy it on this Mac first")
        addrs = dict(sel.machines())
        ssh = [w for w in sel.SSH if w != "-n"]

        def one(name):
            r = run(ssh + ["copal-fleet@%s" % addrs[name], verb], timeout=40, stdin=data)
            lines = (r.stdout.strip() or r.stderr.strip()).splitlines()
            return {"node": name, "ok": r.returncode == 0, "out": lines[-1] if lines else ""}

        with cf.ThreadPoolExecutor(max(1, len(names))) as ex:
            return {"verb": word, "results": list(ex.map(one, names))}

    def shot(self, names):
        d = os.path.join(self.q, "shots")
        os.makedirs(d, exist_ok=True)
        addrs = dict(sel.machines())
        stamp = time.strftime("%Y%m%d-%H%M%S")

        def one(name):
            try:
                r = subprocess.run(sel.SSH + ["copal-fleet@%s" % addrs[name], "shot"], capture_output=True, timeout=40)
            except (subprocess.TimeoutExpired, OSError) as e:
                return {"node": name, "ok": False, "out": str(e)[-120:]}
            if r.returncode != 0 or not r.stdout:
                err = r.stderr.decode(errors="replace").strip().splitlines()
                return {"node": name, "ok": False, "out": err[-1] if err else "nothing came back"}
            kind = "png" if r.stdout[:4] == b"\x89PNG" else "txt"
            p = os.path.join(d, "%s-%s.%s" % (name, stamp, kind))
            with open(p, "wb") as f:
                f.write(r.stdout)
            return {"node": name, "ok": True, "out": p}

        with cf.ThreadPoolExecutor(max(1, len(names))) as ex:
            res = list(ex.map(one, names))
        got = [x["out"] for x in res if x["ok"]]
        if got and sys.platform == "darwin":
            run(["open"] + got, timeout=10)
        return {"verb": "shot", "results": res}

    # ---- Fleet Chat: one room, and this console hosts it --------------------------
    #
    # THE RECORD IS HERE, in QUEUE/chat/room.log, and each machine holds its copy and
    # a position. A round gives each machine what it has not seen and takes what its
    # person typed. THE NAME AND THE TIME ARE WRITTEN HERE, not by the machine: one
    # machine cannot speak as another, and a room of UTC and Pacific clocks has one
    # clock. A machine is never sent its own line, which it already shows as "you".
    def room_path(self, name="room.log"):
        d = os.path.join(self.q, "chat")
        os.makedirs(d, exist_ok=True)
        return os.path.join(d, name)

    def room(self):
        try:
            with open(self.room_path(), errors="replace") as f:
                return f.read().splitlines()
        except OSError:
            return []

    def room_add(self, who, text):
        clean = "".join(c for c in text if c >= " " and c != "\x7f")[:2000]
        if clean:
            with open(self.room_path(), "a") as f:
                f.write("%s %s: %s\n" % (time.strftime("%H:%M"), who, clean))

    def sync(self):
        """One round, under a lock: the page's poll and a press may meet here."""
        with open(self.room_path("lock"), "w") as lk:
            fcntl.flock(lk, fcntl.LOCK_EX)
            try:
                with open(self.room_path("positions.json")) as f:
                    pos = json.load(f)
            except (OSError, ValueError):
                pos = {}
            nodes = sel.machines()
            for _ in range(2):
                lines = self.room()

                def one(n):
                    name, addr = n
                    st = pos.get(name) or {"sent": max(0, len(lines) - 200), "ack": False}
                    mine = [l for l in lines[st["sent"]:] if not l[6:].startswith(name + ": ")]
                    verb = "chat-sync ack" if st.get("ack") else "chat-sync"
                    r = run([w for w in sel.SSH if w != "-n"] + ["copal-fleet@%s" % addr, verb], timeout=15,
                            stdin="".join(l + "\n" for l in mine))
                    if r.returncode != 0:
                        return name, False, st, []
                    return name, True, {"sent": len(lines), "ack": True}, r.stdout.splitlines()

                with cf.ThreadPoolExecutor(len(nodes)) as ex:
                    got = list(ex.map(one, nodes))
                said = False
                ok = {}
                for name, up, st, typed in got:
                    ok[name] = up
                    pos[name] = st
                    for t in typed:
                        self.room_add(name, t)
                        said = True
                # What was just said goes round now rather than a poll from now.
                if not said:
                    break
            with open(self.room_path("positions.json"), "w") as f:
                json.dump(pos, f)
        return ok

    def chat(self):
        ok = self.sync()
        lines = self.room()
        return {"count": len(lines), "lines": lines[-1000:],
                "machines": [{"name": n, "ok": ok.get(n, False)} for n, _ in sel.machines()]}

    def say(self, text):
        with open(self.room_path("lock"), "w") as lk:
            fcntl.flock(lk, fcntl.LOCK_EX)
            self.room_add("orrery", text)
        return self.chat()

    def send(self, names, verb):
        if verb.split()[0] not in VERBS:
            raise SystemExit("no verb called %s" % verb.split()[0])
        addrs = dict(sel.machines())

        def one(name):
            try:
                rc, out, err = sel.ask(addrs[name], verb)
            except Exception as e:
                return {"node": name, "ok": False, "out": str(e)[-160:]}
            text = (out or err).splitlines()
            return {"node": name, "ok": rc == 0, "out": text[0] if text else ""}

        if verb.startswith("message "):
            # A message is a line in the room too, and a window onto it where it lands.
            with open(self.room_path("lock"), "w") as lk:
                fcntl.flock(lk, fcntl.LOCK_EX)
                self.room_add("orrery", verb[len("message "):])
        with cf.ThreadPoolExecutor(max(1, len(names))) as ex:
            res = list(ex.map(one, names))
        if verb.startswith("message "):
            self.sync()
        return {"verb": verb, "results": res}


def mac_clipboard():
    """The console's own clipboard: the way several lines get here at all."""
    for argv in (["pbpaste"], ["wl-paste", "-n"], ["xclip", "-selection", "clipboard", "-o"]):
        r = run(argv, timeout=5)
        if r.returncode == 0:
            return r.stdout
    return ""


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    fl, cmd, rest = Fleet(argv[0]), argv[1], argv[2:]
    if cmd == "status":
        out = fl.status()
    elif cmd == "survey":
        fl.survey()
        out = {"said": "every machine asked"}
    elif cmd == "login":
        out = fl.login()
    elif cmd == "select":
        out = fl.select(rest)
    elif cmd == "send":
        out = fl.send([n for n in rest[0].split(",") if n], " ".join(rest[1:]))
    elif cmd == "shot":
        out = fl.shot([n for n in rest[0].split(",") if n])
    elif cmd == "chat":
        out = fl.chat()
    elif cmd == "say":
        out = fl.say(" ".join(rest[rest.index("--") + 1:]))
    elif cmd == "text":
        cut = rest.index("--") if "--" in rest else len(rest)
        typed = " ".join(rest[cut + 1:])
        out = fl.text([n for n in rest[0].split(",") if n], " ".join(rest[1:cut]), typed or mac_clipboard())
    else:
        raise SystemExit("no command called %s" % cmd)
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
