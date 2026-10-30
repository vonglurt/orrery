# Fleet Commanders: From Timbuktu's Buttons to a Listener, and Selecting Machines by What They Are

*Lab Report — IEEE Format · 30 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `tools/fleet-listener.sh`, `tools/fleet-exec.sh`,
`tools/fleet-listener-check.sh`, `tools/fleet-select.py`; `src/verbs.rs`,
`src/lab.rs`; `tools/machines.py` (the recipe).

---

## Abstract

Paul selected three machines in Watch → Lab, pressed Message, and nothing
reached them. He asked for three things:
- the Lab's buttons working: Controls, Terminal, Exchange, Send, Message,
  Run, Scene, Snapshot, Power;
- a listener on Ubuntu and Alpine to receive them, and a login message that
  reads the last message sent to the fleet;
- a report on this kind of fleet commander, with a method for selecting
  machines "by ubuntu or alpine or by idle, or by version, or by contains
  package or capability (like date on treadcore code and last commit)".

The findings:
- **The buttons had nothing to talk to.** The button stopped at `no operator
  certificate`, and past that no node had the account, the forced command, the
  CA or a host certificate. The design existed and was never deployed here.
- **The listener is sshd, not a new daemon.** A new port would be a second
  thing to authenticate. One root script installs a forced command and a
  certificate check, on both systems, and `--undo` removes it.
- **38 checks pass on stock Alpine 3.22 and Ubuntu 24.04**, against a real
  sshd and real certificates. Nearly half are things that must be refused.
- **Selection should ask the machine.** A selector, `fleet-select.py`, reads
  live facts through the same forced command. Nothing is written down to go
  stale, which is the failure that started this morning: two machines were
  called down because a list still held yesterday's addresses.
- **Two buttons do not fully work and this report says so:** Control has no
  server to connect to, and Snapshot has nothing to restore (§V).

---

## I. Why Timbuktu

Timbuktu began as a Macintosh remote-control program from WOS Data Systems,
which Farallon bought in 1988 [1]. Timbuktu Pro followed in late 1993 and added
Windows control and TCP/IP [3]. Farallon's software went on with Netopia, which
Motorola bought in 2007 [1]; it is now listed as discontinued [2].

Three things about it are worth keeping:

- **Verbs were the interface.** A 2000 review lists what you could do to
  another computer: send a pop-up message, chat by text or voice, observe its
  screen, take control of it [4]. Wikipedia adds file transfer and system
  profiling [2]. The Lab's bar is this list, and a person reads it as a list of
  things they can do rather than a list of protocols.
- **Several at once.** The same review singles out multiple simultaneous
  connections as what set it apart [4]. That is the Lab's Sessions grid.
- **It was one-to-one at heart.** Every verb assumed you knew which machine you
  meant. Twenty years of tools since have mostly been about the other question:
  *which machines?*

The exact names of Timbuktu's services (Control, Observe, Send, Exchange, Notify,
Intercom, Profile) are from memory. The sources above confirm the functions and
do not spell out that list, so it should not be quoted as documented.

## II. The Commanders Since

| Tool | How it picks machines | How a command arrives | What it taught |
|---|---|---|---|
| Timbuktu | you choose one, or several | its own protocol and service | verbs a person can read |
| Apple Remote Desktop | **Smart Lists**: a list that fills itself from "any" or "all" of criteria such as name, address, OS version, memory, CPU, model [5] | its own agent | selection as a saved question |
| Ansible | inventory, then `group_by` from gathered facts; groups exist only for that run [6]; a limit intersects groups [7] | ssh, nothing installed | facts decide; no daemon needed |
| Salt | **grains**, `os:Ubuntu`, and compound expressions combining matchers with `-C` [8] | its own agent | a small language for "which" |

**The line of development is that selection moved from a name to a question.**
ARD's reviewer in 2006 found the criteria limited [5], and they are: ARD can ask
about memory and CPU, and cannot ask what commit a checkout is on. The fleet
needs exactly that question.

## III. The Listener

### A. What was missing

Reproduced from the Mac, on columbia:

```
$ copal fleet run --fleet grove --node columbia message test
error: no operator certificate. Run: copal fleet login
```

Behind that, on every node: no `copal-fleet` account, no forced command, no CA,
no host certificate. `copal-prep.sh` builds all of these when it makes a Copal
card, and these machines did not come from that stage. It also assumes OpenRC
and doas, so it could not simply be run on Ubuntu.

### B. Weighed

| Choice | For | Against | |
|---|---|---|---|
| New daemon on its own port | one binary, any protocol | a second thing to authenticate and patch; root or a rule anyway | rejected |
| Reuse the user's ssh key and run commands | nothing to install | any command runs as the user; that is a shell, not a verb list | rejected |
| **sshd, a certificate, a forced command** | the fleet's CA already decides; the verb list is one file; the console already speaks it | 8-hour certificates must be renewed | **taken** |

### C. What the script does

`tools/fleet-listener.sh`, once, as root. It stages from Machines → *Answer the
Lab's buttons*, which also signs the machine's host certificate on the Mac; the
CA's private key never travels.

- **The account `copal-fleet`:** password `*`, no tty, no forwarding, every login
  sent to `/usr/bin/copal-fleet-exec`.
- **Certificates by purpose:** `fleet-operator` reaches the forced command;
  `fleet-human` reaches the user's shell, for Terminal and Exchange. The user's
  ordinary key goes on working, so nothing the queue does changes.
- **The certificate is proved before sshd hears of it:** a host certificate, for
  this machine's key, from this CA. Otherwise the script refuses, having changed
  nothing.
- **The block is proved after:** one marked block at the bottom of
  `sshd_config`, `sshd -t`, or the old file back. sshd is reloaded and not
  restarted, so open sessions and running jobs stay up.
- **The rule is narrow:** the account may run one power helper as root and one
  read-only facts helper as the user. Each rule passes `doas -C` or `visudo -c`
  before it counts.

### D. The last message

`message TEXT` writes `/var/lib/copal-fleet/last-message` **before anyone is told**,
so a machine nobody is sitting at still has the message at the next login. It then
tries `wall` and the desktop's notifications; neither can fail the verb.

The login reads the file: `/etc/update-motd.d/02-copal-message` on Ubuntu, which
builds its motd dynamically, and `/etc/profile.d/copal-message.sh` on Alpine,
which prints a fixed file. The text is the console's own allow-list, letters,
digits, spaces and `. , ! ? : -`, spelled again on the node.

## IV. Results

`tools/fleet-listener-check.sh`, in a stock container of each kind, against a real
sshd:

| | Alpine 3.22 | Ubuntu 24.04 |
|---|---|---|
| Checks passed | 38 of 38 | 38 of 38 |

**Must work:** the host presents its certificate; `id`, `uptime`, `facts`, `has`;
a scene is recorded; a message is kept and read back by file, verb, motd and facts;
a human's certificate reaches the user's shell; run twice leaves one block; undo
removes the block and the account and leaves a file sshd accepts.

**Must be refused:** an unknown verb; a shell; a message with a quote, `$(id)` or a
backtick; over 200 characters; a scene with a path; `has` with a path; a tty; a
forward carrying a real connection; a certificate for another principal; one from
another CA; a human's certificate at root; a host certificate for another machine's
key; `doas`/`sudo` reaching `cat` through the rule.

The first run failed 8 checks on Alpine and 2 on Ubuntu. All were faults in the test: a line it
wrongly added to Alpine's `doas.conf`, a tty check expecting the wrong message, and
a forwarding check that never sent anything through the forward. The last is the
one worth remembering: it passed nothing and proved nothing until it carried
traffic.

The console's suite passes, 354 tests. Scene's argv changed from `scene NAME` to
`run scene apply NAME`, because `copal fleet scene` runs a playbook from a checkout
these machines have no use for.

**Not yet measured:** the listener on the eight real machines, and the Lab's buttons
against them. It is staged on all eight and needs the root password typed once on
each. This section is to be extended with that run.

## V. The Buttons

| Button | Reaches | State |
|---|---|---|
| Message | `message TEXT`; file, wall, desktop; motd | built, tested in containers |
| Run | `run WORD`: `id`, `uptime`, `facts`, `log` | built, tested in containers |
| Scene | `scene apply NAME`: **records** a label, applies nothing | built, tested in containers |
| Power | off and reboot through the helper; **on** still needs `fleet.toml` and a switched port | off and reboot built; the helper tested, a real power-off not |
| Terminal | the user's shell, on a `fleet-human` certificate | built; needs the real machines |
| Exchange, Send | SFTP on the same certificate | built; needs the real machines |
| **Control** | RDP to a node's screen | **does not work: no server** |
| **Snapshot** | `snapshot restore` | **refuses: nothing takes snapshots** |

**Control needs a decision per machine.** The Lab's Control speaks RDP.
- boburner has GNOME and `grdctl`, so its built-in RDP server can be enabled.
- calber and whitewolf run no desktop, so there is no screen to serve.
- The five Copal machines run Hyprland, whose screen `x11vnc` cannot see; they need
  an RDP server for Wayland, or Control needs to learn VNC with `wayvnc`.

**Snapshot refuses on purpose.** Its one verb is a restore. These machines hold
weeks of training state and no snapshot exists to restore. A button that does
something else under that name would be worse than a refusal. A real one needs a
filesystem decision first: ZFS or btrfs where those are the root, not on the rest.

**Certificates last 8 hours.** After that every button answers `no operator
certificate` until `copal fleet login` runs again. Having the console mint its own
would cross the line `docs/lockdown.md` draws, so it is left as a step.

**The line is kept.** `docs/lockdown.md` §5 says orrery never creates a credential.
A first version of the recipe signed host certificates from `tools/machines.py`, which
broke that. Signing now lives in `copal fleet certify`, and the recipe only carries the
result.

## VI. Selecting Machines

### A. The method

`tools/fleet-select.py` takes words, ANDed; `or` starts another group.

```
fleet-select.py os=ubuntu
fleet-select.py os=alpine idle
fleet-select.py 'version>=24.04' has:nvidia-smi
fleet-select.py 'treadcore>=2026-10-29' 'mem_gb>=16'
fleet-select.py 'treadcore!=37b6b65'
fleet-select.py os=ubuntu or os=alpine idle --table
fleet-select.py idle --run uptime
```

| Paul asked for | Word | Answered by |
|---|---|---|
| ubuntu or alpine | `os=ubuntu` | `/etc/os-release` |
| idle | `idle` | load under a quarter of the processors, nobody logged in |
| version | `version>=24.04`, `kernel>=6.18` | numbers compared, not text |
| contains a package | `has:tmux` | `apk`, `dpkg-query`, or the program on the path |
| capability | `gpu`, `driver>=580` | `nvidia-smi` |
| date of the treadcore code | `treadcore>=2026-10-29` | `git log` on the checkout |
| last commit | `treadcore=37b6b65` | the same; a prefix matches |

### B. Three decisions

- **It asks; it does not remember.** A saved list is what called two healthy
  machines down this morning. A fleet of nine can afford to ask each time; a fleet
  of nine thousand could not, which is what the larger tools are built around.
- **A machine that does not answer matches nothing, and is named on stderr.** A
  machine nobody could ask is not a machine that matched, and it must not silently
  drop out of a `!=`.
- **A commit word and a date word are the same key.** `treadcore` holds both, and
  the shape of what you typed says which you meant.

### C. The trap in "idle"

**Today no machine will select as idle**, and the selector is right. The queue keeps
every lane full at low priority, so load is high everywhere while the machines would
give the work up at once. "Idle" has two meanings: *nothing is running*, which is
what is built, and *nothing would mind*, which needs the queue's own view of a lane.
The second is probably the one that is wanted for most purposes and it is not built.

### D. Still open

- **Selection is on the Fleet screen, not yet the Lab** (`docs/fleet-orchestration-lab-report.md`).
- **A saved question,** ARD's Smart List: a name for `os=ubuntu has:nvidia-smi`.
- **Facts through the Lab's own state document,** so the tiles can show them.
- **Control and Snapshot,** per §V.

## VII. Acknowledgment and References

The history in §I rests on the sources below, which disagree in places: [1] and [2]
give different dates for Farallon's renaming, so none is given here.

[1] "Netopia," Wikipedia. https://en.wikipedia.org/wiki/Netopia

[2] "Timbuktu (software)," Wikipedia. https://en.wikipedia.org/wiki/Timbuktu_(software)

[3] "Timbuktu Pro," TidBITS, 29 Aug. 1994. https://tidbits.com/1994/08/29/timbuktu-pro/

[4] "Go Timbuktu remote control," Computerworld. https://www2.computerworld.com.au/article/93333/go_timbuktu_remote_control/

[5] "Manage computer lists with Remote Desktop," Apple Support. https://support.apple.com/guide/remote-desktop/apdb313dc09/mac ; review, R. Buckley, MacFormat, 2006. https://www.robbuckley.co.uk/galleries/macformat/review-apple-remote-desktop-3.php?page=2

[6] "How to use Ansible group_by to create dynamic groups from facts," OneUptime. https://oneuptime.com/blog/post/2026-02-21-how-to-use-ansible-group-by-to-create-dynamic-groups-from-facts/markdown

[7] "Create dynamic groups with constructed inventories," Red Hat. https://docs.redhat.com/it/documentation/red_hat_ansible_automation_platform/2.6/administer-ref_controller_constructed_inventories

[8] "salt.modules.match," Salt Project. https://docs.saltproject.io/en/3006/ref/modules/all/salt.modules.match.html
