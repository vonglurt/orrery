# fleet-train, and the Training view

*The fleet used as one computer: rounds of work on every spare CPU and the
GPU, each round judged, watched from the museum interface. First use:
training treadcore's learned tank on calber's CUDA card (treadcore's
LAB-REPORT-BRAIN.md sections 108 and 109; LAB-REPORT-FLEET.md).*

## Run it, on the coordinator (this Mac)

```sh
cd ~/code/treadcore
~/code/orrery/tools/fleet-train.sh run    scripts/fleet-train-stages.sh       # rounds until stopped
~/code/orrery/tools/fleet-train.sh run    scripts/fleet-train-stages.sh 1     # one round
~/code/orrery/tools/fleet-train.sh status scripts/fleet-train-stages.sh       # where it is
~/code/orrery/tools/fleet-train.sh stop   scripts/fleet-train-stages.sh       # after the round in hand
```

## Watch it: the Training view

```sh
cd ~/code/orrery
cargo run --release -- --gui --fleet grove --fleet-cmd "$HOME/.local/bin/copal fleet" \
  --training-cmd "python3 -I $HOME/code/orrery/tools/plans/treadcore-view.py"
```

The fleet is **grove**, the real one: saskatchewan, columbia, nasser and
calber, declared in `~/.copal/fleets/grove/nodes` (the Mac has no avahi, so
the list is written down). `copal fleet` on the Mac comes from
`make install-fleet` in copal-alpine-linux. The **Lab** view shows the four
machines as the fleet CLI sees them; they run no fleet agent yet, so their
tiles say "off bus" until they are enrolled.

**Training** (Command-3) is laid out to be read:

- **the top line** says what is happening, in a sentence, and the round's
  three steps light in turn: record, train, judge;
- **the fleet**, a card a machine: what it is for, its CPU load against its
  cores, calber's GPU, what it is doing now in words, and the command doing
  it;
- **the net being trained**: its three weight matrices as heat maps (green
  positive, red negative, stronger is larger, with a legend), **click one to
  enlarge it**: its axes say what the inputs and outputs are (the senses by
  group, the units, the scores by head), a legend runs from its most negative
  weight to its most positive, and the cell under the pointer reads out its
  weight; Escape or Close goes back. Then the 47 scores split into the
  heads they choose for, how often it makes the scripted brain's choice on
  matches it never saw (against the first clone and against always the
  commonest choice), and its census against the official net.

![The Training view while rec2400 records](features/training.png)

It reads one document, printed by `tools/plans/treadcore-view.py` every
5 s: each machine asked over ssh for its cores, load, memory and GPU; the
newest recording run's lanes; the loop's state; the best net's weights; its
results. `--frame out.ppm --frame-view training --frame-state doc.json`
draws a saved document, for review (`ORRERY_FRAME_ZOOM=N` with a matrix
enlarged). Text is drawn a pixel bolder than the font; `--thin` goes back.

## The queue: orrery runs treadcore's training (fleet-queue)

Orrery holds all of the orchestration (treadcore LAB-REPORT-BRAIN.md 112
and 113):
- **the engine,** `tools/fleet-queue.py`: the queue, a lane a machine, the
  keeper, the stops, sync and shutdown;
- **the plan,** `tools/plans/treadcore.py`: what the fleet does for
  treadcore, and how it gathers what came back;
- **the view,** `tools/plans/treadcore-view.py`: what the Training view
  draws.

treadcore keeps only the work itself: how one census plays
(`scripts/census.sh`), the brain's tests, the ledger and the fit
(`scripts/ledger.py`, `scripts/suggest.py`), and the trainer (`tools/bc`).

### 1. The standing queue, grove

`~/code/treadcore/build/queue/grove/` is the fleet's queue. It holds two
files.

**`config.json` says who works**, how each machine powers off for a
shutdown, and how often to gather:

```json
{"hosts": [
  {"name": "saskatchewan", "target": "user@192.168.8.235", "caps": ["cpu"], "nice": 10},
  {"name": "calber",   "target": "user@192.168.8.181", "caps": ["cpu", "gpu"], "nice": 10,
   "poweroff": "sudo -n /usr/bin/systemctl poweroff"},
  {"name": "mac",      "target": "-", "caps": ["cpu"], "nice": 19}],
 "planner": "python3 -I /Users/user/code/orrery/tools/plans/treadcore.py /Users/user/code/treadcore/build/queue/grove",
 "aliases": {"users-iMac": "mac", "users-iMac.lan": "mac", "iMac.lan": "mac"},
 "low_water": 7, "poll_s": 10, "sync_s": 1800}
```

**`plan.json` says what to train:**

```json
{"project": "~/code/treadcore",
 "commit": "origin/main",
 "census_host": "saskatchewan",
 "history": "build/ledger/fleet.tsv",
 "weights": {"seeds": 55, "secs": 900, "map": "all", "seed0": 20000000,
             "held_seeds": 400, "held_seed0": 60000000},
 "families": {"converge": ["build/fleet-sync/saskatchewan/converge/20261026-181011"], "queue": true, "batch": 8}}
```

- **`commit`.** `origin/main` is read again, after a fetch, at every
  top-up. So the fleet always works on the newest commit this Mac pushed.
  - Each machine builds each commit once, in `~/fleet/q-grove/c/SHORT/`.
  - Before building, a machine fast-forwards its own checkout
    (`~/code/treadcore`), when that checkout is clean and on main.
  - New jobs use the newest build. A job already planned runs on its own.
- **`history`.** The search starts from every census the fleet has read
  that shares the queue's settings (55 seeds or more, 900 s, every map)
  and today's `weights.rs`. A promotion changes `weights.rs`, so later
  trials are measured against the new defaults and are never mixed with
  the old.
- **`net`.** It adds rounds of the learned tank (see sections 3 and 4 of
  the first queue's settings in treadcore LR 110); grove has none for
  now.

### 2. Open it in orrery

```sh
cd ~/code/orrery && cargo run --release -- --gui --operator paul --fleet grove \
  --fleet-cmd "$HOME/.local/bin/copal fleet" --queue $HOME/code/treadcore/build/queue/grove
```

`--queue DIR` sets both what the Training view reads and what its buttons
run. Training is Command-3.

**The view shows:**
- the queue's state, in its top sentence;
- a bar of done (green), running (amber) and failed (red);
- the counts for each kind of job, the next job and the last finished;
- the last sync;
- what each machine is running.

**For an operator, the buttons are:**
- **Stop adding**, or **Resume adding** while draining;
- **Collect results and stop now**;
- **Start the queue**, when its keeper is not running;
- **Sync now:** gather every machine's record at once;
- **Shut the fleet down.** It asks twice, because the second press powers
  machines off:
  1. nothing new starts;
  2. each machine finishes the job in hand, and its results and its ledger
     come back;
  3. then it powers off;
  4. a report, `shutdown.json`, says who finished what and what never
     started. Its line is shown under the bar.

The same from a shell:

```sh
Q=~/code/treadcore/build/queue/grove; F="python3 $HOME/code/orrery/tools/fleet-queue.py $Q"
$F ensure; $F status; $F stop-adding; $F resume-adding; $F stop-now; $F sync; $F shutdown-now
```

### 3. Gathering: sync

**The keeper runs `PLANNER sync` every `sync_s` seconds, and once more at
the end.** It gathers, with no command by hand:
1. Each machine's own ledger, read by `scripts/ledger.py` on that machine,
   so each row keeps its census binary's hash. The ledgers go to
   `~/code/treadcore/build/ledger/HOST.tsv`.
2. Each machine's converge records, without their binaries, into
   `build/fleet-sync/HOST/converge/`.
3. Every queue's own ledger.

**All of it is joined into `build/ledger/fleet.tsv`**, one row a machine
and census:
- rows are joined, never pooled: a census belongs to its machine;
- the Mac's names (users-iMac, users-iMac.lan) are one, by `aliases`;
- the last sync is in `build/ledger/sync.json`.

A machine that cannot be reached keeps the table it had.

### 4. What it found

All of it is under `$Q`:

| File | What it holds |
| --- | --- |
| `plan/summary.json` | written when the queue stops: every weight census and whether it passed, every net round, the best net, the families |
| `plan/ledger.tsv` | the queue's own censuses; `plan/history.tsv` the fleet's rows it also fits; `plan/ledger-all.tsv` both |
| `plan/families.json` | every candidate's families: each family's share, error and floor, and `holds` or `UNDER` |
| `plan/round-NNN.json`, `plan/best.bin` | net rounds, when the plan has them |
| `plan/state.json` | the planner's counters, and each commit seen with its `weights.rs` |
| `shutdown.json` | after a shutdown: each machine's last job, whether it reported, whether it is off; the jobs never started |
| `logs/ID.log`, `results/ID/` | each job's commands and output; what it brought back |
| `events.jsonl` | every start, finish, failure, requeue, sync, report and power-off |

**A weight trial that passes** is read twice more on 400 held-out seeds.
One that passes both is a candidate. Its families run next, with the
families of every converge run in `families.converge`, newest first, 8 to
a job side by side.

Nothing becomes a default, and no net becomes official, without Paul.

### 5. When a machine goes away, joins, or must power off

- **A host that ssh cannot reach takes nothing.** Its lane says
  `unreachable` and probes it each minute.
- **A job that ended because ssh failed (255)** on a host that is then
  unreachable goes back on the queue for another host. It is not failed,
  and the breaker does not count it.
- **To take a host out,** delete it from `config.json`. Its lane ends after
  the job in hand.
- **To add one,** add it. Its lane starts, and builds the commit first. A
  host with `"caps": []` takes no work, but its record is still gathered.
- **To power off at a shutdown,** a host needs `poweroff`, a command it may
  run without a password. A host without it is left running, and the
  report says so. `tools/fleet-poweroff-setup.sh` writes the one rule that
  allows it: run it once on each machine.

## What it is

| Piece | Where | What it does |
| --- | --- | --- |
| the loop | `tools/fleet-train.sh` (here) | rounds, stages in order, an event a line (`events.jsonl`), STOP, `status --json` |
| the stages | the project's (`treadcore/scripts/fleet-train-stages.sh`) | `stage_record`, `stage_train`, `stage_judge`, and `ft_detail` for the view |
| the recording | `treadcore/scripts/fleet-record.sh` | a queue of jobs claimed by `mv`, a lane a machine, shards streamed to the GPU node and hashed at both ends |
| the queue | `tools/fleet-queue.py` (here) | jobs, a lane a machine, the keeper, stop adding, stop now, sync, shutdown |
| the plan for treadcore | `tools/plans/treadcore.py` (here) | prepares each commit on each machine; weight trials, held-out reads, families, net rounds; sync |
| the lanes as JSON | `tools/plans/treadcore-jobs.py` (here) | each machine's job, state, age and the pipe it is running |
| what is shown | `tools/plans/treadcore-view.py` (here) | the machines, the queue, the run, the loop, the net and its results, as one document |
| counting | `tools/plans/treadcore-count.py` (here) | the games, seat-ticks, thinks and training a queue has done |
| the view | `src/training.rs`, the Training face | draws that document; it has no verbs |

## The rules it keeps

1. **The same read rule as every face.** The Training view draws one command's
   document and nothing else; a job that is not in it cannot be drawn.
2. **Watching and steering are two things.** The view's buttons exist only
   for an operator (`--operator`), and only with a queue to drive.
3. **A failure stops the loop.** A stage that fails ends the round as
   failed; the loop does not retry around it.
4. **A round's best is the loop's, not the project's.** treadcore's judge
   promotes a candidate only if the census, paired by seed on one binary,
   puts it above the best outside two standard errors, and even then the
   official net (`nets/map7.bin`) changes only when Paul says.
5. **Spare means spare.** The hosts file names each machine's threads and
   nice; a machine busy with its own work is left out.
