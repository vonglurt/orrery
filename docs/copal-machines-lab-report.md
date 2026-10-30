# Two Copal Machines Join the Fleet: Keys, Setup, Errands and Updates by Themselves

*Lab Report — IEEE Format · 29 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `docs/fleet-train.md` (the machines' bullets); `tools/machines.py`;
`tools/copal-node.sh`, `fleet-update.sh`, `fleet-auto-update.sh`,
`fleet-pull.sh`; `src/machines.rs`, `src/media.rs`. Commits `4daa764`,
`acbfc5e`.

---

## Abstract

Two machines with Copal's full install were added to the fleet:
- **xinanjiang** (192.168.8.217): an i5-7200U laptop, 4 threads, 7 GB;
- **jaguaribe** (192.168.8.245): two Xeon X5670s, 24 threads, 47 GB.

Copal is locked down to SSH keys, and only saskatchewan's key was on
them. Paul's own key reached them through saskatchewan, with no private
key needing to move. Both joined the queue as CPU lanes. jaguaribe now
reads 961 seeds an hour, about 18% more for the whole fleet.

Joining exposed three faults in orrery:
1. **Every setup recipe was Ubuntu's.** A Copal machine had no button
   but power-off, and Join sent it to a recipe marked "not for it".
2. **The queue knew one machine's power-off command, whitewolf's.** Join
   had copied each command once. A rule applied after joining was never
   seen, so a fleet shutdown would have left five machines running.
3. **The Media pane stopped at "the card ledger cannot be read"** when
   `answers.txt` described a standalone machine, with no way forward.

Each is fixed. There is a Copal setup recipe, two errands on every
machine (update the packages, pull every project), and a recipe that
keeps a machine up to date by itself, daily and at boot. The probe now
keeps the power-off command current, and the Media pane walks through
the steps. All eight Linux machines are now set to update daily and
have a working power-off command.

## I. Objective

1. Let Paul's Mac log into the two new machines.
2. Make them fleet nodes and join them to the queue.
3. Give a Copal machine the same setup buttons an Ubuntu machine has, and
   make every button in the Machines view lead somewhere.
4. Add errands for every machine: update the packages, pull the projects.
5. Keep every machine up to date by itself, daily and after a boot.
6. Turn the Media pane's dead end into steps a person can follow.

## II. Materials

| Item | Detail |
| --- | --- |
| New machines | xinanjiang, i5-7200U, 4 threads, 7 GB; jaguaribe, 2 × Xeon X5670, 24 threads, 47 GB, 438 GB disk; both Alpine 3.24 (Copal's full install), Rust 1.96.1, git, tmux, doas |
| The fleet | saskatchewan, columbia, nasser (Copal); boburner, calber, whitewolf (Ubuntu 24.04, NVIDIA, CUDA 12.9, driver 580); the Mac (the coordinator) |
| Test beds | colima on the Mac: `alpine:3.22` with OpenRC, `ubuntu:24.04` with systemd |
| The app | orrery's Machines view (`machines.rs`) and Media pane (`media.rs`); frames rendered with `--frame` |

## III. The key, through the one machine that was let in

The new machines trusted saskatchewan's key only. The method used sends
the Mac's **public** key along that trusted path:

```
ssh user@192.168.8.235 "ssh user@192.168.8.217 'cat >> ~/.ssh/authorized_keys'" < ~/.ssh/id_ed25519.pub
```

The Mac pipes its public key to saskatchewan. Saskatchewan logs into the
new machine with its own key and appends what it was given. After that
the Mac logs in directly.

Three attempts failed before this one, each on how a command was typed,
not on the lockdown:
- the loop was run on saskatchewan, so it tried to log into itself;
- a pasted line broke after `cat >>`, so the remote shell received an
  unfinished command;
- the two addresses were swapped, so the outer hop went to the machine
  that did not yet know the key.

One attempt did succeed unintentionally: the Mac's private key was copied
to saskatchewan as `~/.ssh/id_mac`. That copy is not needed, and it
should be removed (`ssh user@192.168.8.235 'rm ~/.ssh/id_mac'`).

**The lesson for the instructions:** commands pasted from a wrapped
message pick up line breaks. The working forms were one line, and short:
`ssh -t user@192.168.8.$h doas sh orrery-recipes/fleet-auto-update.sh`.

## IV. Making a Copal machine a node

**What a node needs.** Rust, git, tmux, the treadcore checkout, `~/fleet`
and the power-off rule. Copal's full install has all of these but the
checkout, `~/fleet` and the rule. For this first pair the checkout was
cloned by hand, and the two joined at once.

**The recipe** `copal-node.sh` (**Make it a fleet node (Copal)**) does the
same on any Copal machine. Each step is checked first, so running it
again is safe:
1. it installs the build tools missing from Alpine's own repositories;
2. as the user, it clones treadcore over HTTPS and makes `~/fleet`;
3. it adds the power-off rule.

It changes nothing else: no service, firewall or SSH setting, so
Copal's lockdown stays as it is. On both new machines it reported "the
build tools are there", found the checkout, and wrote the rule.

**Join's advice** now names the recipe that fits the machine's system:
`copal-node` on Alpine, `fleet-node` on Ubuntu.

## V. The power-off command, which only one machine had

A fleet shutdown (`shutdown-now`) powers each machine off by the command
in that machine's `config.json` entry. It leaves a machine without one
running, and says so.

Join copied the command into that entry at the moment of joining, from
the probe's reading. A rule applied after joining was never copied. When
every machine was probed on 29 October, the queue held a command for
whitewolf alone. Saskatchewan, boburner, columbia, calber and nasser all
had working rules, and the queue would have left every one of them
running.

**The fix** (`sync_poweroff`): every probe writes the command the
machine's rule allows into its entry, and removes it if the rule is gone.
After one probe of all machines, every machine but the Mac has its
command.

## VI. Errands: update the packages, pull the projects

| Button | Script | Root | What it does |
| --- | --- | --- | --- |
| Update the system's packages, **Run…** | `fleet-update.sh` | yes, typed in the machine's Sessions tile | `apk update` and `apk upgrade`, or `apt-get update` and `upgrade` |
| **Pull projects** | `fleet-pull.sh`, fed over ssh on stdin | no | every git checkout in the home, fast-forward only |

**An errand is not a state.** It has no "applied", only a last run, so the
recipe's state reads "ready to run" and its button says **Run…**, not
**Apply…**.

**Pull never merges, rebases, stashes or resets.** A checkout with
changes of its own, no upstream, or a branch that has diverged is left
as it is, and its line says so. The script is sent on stdin, so nothing
is left on the machine. Its lines go into the machine's log under "Done
here". jaguaribe's first pull: `code/treadcore: up to date (37b6b65)`.

**Neither restarts anything.** A new kernel waits for the next boot, and
the output says so.

## VII. Up to date by itself, daily and at boot

`fleet-auto-update.sh` (**Update daily and at boot**) is run once as
root. It installs `fleet-update.sh --auto` at `/usr/local/sbin` and
schedules it:

| System | Daily | After a boot |
| --- | --- | --- |
| Alpine (Copal) | busybox crond's `/etc/periodic/daily`, at 2:00; the root crontab is given Alpine's daily line if it lacks one | OpenRC's `local` service, `/etc/local.d/fleet-update.start`, after 2 minutes, in the background |
| Ubuntu | a systemd timer at 3:00, spread over an hour | the same timer, 5 minutes after the boot; a day missed while off is made up (`Persistent`) |

**The automatic runs hold back the NVIDIA driver and CUDA on Ubuntu.** A
driver upgraded under a running machine leaves its library out of step
with the module loaded. Every CUDA program then fails until the boot,
with "Driver/library version mismatch". Crucible's lanes would stop
overnight for an update no one chose that day. The packages are held for
the run and released after it. A package held before the run stays held.
A by-hand Run upgrades everything.

**Each run leaves one line** in `/var/log/fleet-update.log`. The probe
shows it as **updated**, and the daily job as **daily update**.
`--undo` takes it all away.

### Tests

In containers on the Mac, before any machine ran it:

| Test | Alpine 3.22 | Ubuntu 24.04 |
| --- | --- | --- |
| install | the files written, `local` in the default runlevel | the units pass `systemd-analyze verify` |
| an automatic run, a by-hand run | both up to date, both lines logged | up to date, logged |
| install again | the same files, no change | — |
| a root crontab with no daily line | Alpine's line added, once | — |
| `--undo` | every file gone, the log kept | — |
| the NVIDIA hold | — | a stand-in `nvidia-dummy-580` held during the run and released; a `libc6` held before stayed held |

**The first test caught a fault.** Busybox's `install` does not create a
missing directory, and `/usr/local/sbin` did not exist. That turned out
to be true on Copal too (saskatchewan and jaguaribe have none). Without
the test the recipe would have failed on every Copal machine. It now
makes the directory.

**On the machines:** all five Copal machines reported the daily job and
the boot run, and all three Ubuntu machines enabled their timers. The
Ubuntu timers fired at once, since each machine had been up longer than
five minutes. Each reported "apt up to date", with the NVIDIA packages
held back (65 on boburner, 49 on calber and whitewolf) and released.
calber's card still reports driver 580.178.04. The Copal machines' first
run is at 2:00.

## VIII. Media: the steps where there was a dead end

`answers.txt` in `~/code/copal-alpine-linux` had `COPAL_FLEET` empty: it
described one standalone machine. The pane read that as "the card ledger
cannot be read" and stopped.

It now shows the five steps, the one to take now lit:
1. the checkout (done);
2. **`make answers`**, the interview naming the fleet;
3. the image (`make img-BOARD`);
4. writing card 1 (`make sd-BOARD`, `ERASE` typed twice);
5. the next card (`make answers-node N=2`).

**Start the interview** runs `make answers` in the pane's own terminal,
the one `ERASE` already reaches. So the questions are answered by the
operator, typed there, and nothing is answered on their behalf. **Show
what is on file** runs `make answers-show` first, so a standalone answers
file is seen before the interview replaces it. Once a fleet is named, the
card table and its existing buttons take steps 3 to 5.

Rendered with `--frame`, the first version cut off three lines at the
right edge (the fault `docs/` calls the drawing traps), and they were
shortened. The interview itself has not yet been run through to the end:
its answers are the operator's to type.

## IX. Results

| Machine | Daily and at boot | Power-off in the queue | Recipes left |
| --- | --- | --- | --- |
| saskatchewan | yes | `doas -n /sbin/poweroff` | none |
| columbia | yes | `doas -n /sbin/poweroff` | none |
| nasser | yes | `doas -n /sbin/poweroff` | none |
| xinanjiang | yes | `doas -n /sbin/poweroff` | none |
| jaguaribe | yes | `doas -n /sbin/poweroff` | none |
| boburner | yes | `sudo -n /usr/bin/systemctl poweroff` | none |
| calber | yes | `sudo -n /usr/bin/systemctl poweroff` | none |
| whitewolf | yes | `sudo -n /usr/bin/systemctl poweroff` | none |

| Measure | Value |
| --- | --- |
| Lanes in the queue | 12, all running (9 CPU, 3 CUDA) |
| jaguaribe's census rate | 961 seeds an hour (saskatchewan 2,158), about 18% of the fleet's before it |
| xinanjiang | built treadcore in 98 s; no census finished yet |
| orrery's tests | 352 pass |

## X. Discussion

**A button that is absent is only honest if its absence has a way
forward.** The Machines view draws no button for a recipe that does not
fit, and that is right: a greyed button is a promise it cannot keep.
But a Copal machine then had nothing to press, and Join's own error sent
the operator to a recipe the view had just said was "not for it". The
fault was not the absent button. It was that no recipe fitted. Every
system the fleet runs now has one.

**A copy taken once is a copy that goes stale.** Join copied the
power-off command as a convenience, and nothing ever looked at it again.
The cost stayed invisible until a shutdown would have left five machines
on. The probe already read the truth every time it ran. The fix was to
keep the copy in step with it, not to read it more carefully once.

**An automatic update is a change no one chose that day.** So the
automatic runs do less than a person pressing Run. They never restart
anything, and they leave alone the one package set whose upgrade breaks a
running GPU. The by-hand run does everything, because a person running
it is there to restart.

**Still open:**
- **Kernel modules on Alpine.** An Alpine kernel upgrade replaces the
  running kernel's modules. Until the boot, a module not already loaded
  (a USB device plugged in later) cannot load. The log says a boot is
  due. The fleet does not yet act on that line.
- **Probe on a schedule.** The Machines view shows the last update only
  as of its last probe, and nothing probes on a schedule yet.
- **The copied private key** `~/.ssh/id_mac` on saskatchewan should be
  removed.

## References

[1] Alpine Linux wiki, "Alpine Linux package management" (`apk update`,
`apk upgrade`) and "cron" (busybox crond, `/etc/periodic`).

[2] OpenRC, "local" service: `/etc/local.d/*.start` at boot.

[3] systemd, `systemd.timer(5)`: `OnCalendar`, `OnBootSec`,
`RandomizedDelaySec`, `Persistent`.

[4] Debian, `apt-mark(8)`: `hold`, `unhold`, `showhold`.

[5] NVIDIA, "Driver/library version mismatch": the NVML error after a
driver upgrade without a reboot.

[6] orrery `docs/fleet-train.md`, the machines' bullets: Copal nodes,
errands, daily updates, the power-off command.
