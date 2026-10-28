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
  --training-cmd "cd ~/code/treadcore && python3 -I scripts/fleet-view.py"
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

It reads one document, printed by treadcore's `scripts/fleet-view.py` every
5 s: each machine asked over ssh for its cores, load, memory and GPU; the
newest recording run's lanes; the loop's state; the best net's weights; its
results. `--frame out.ppm --frame-view training --frame-state doc.json`
draws a saved document, for review (`ORRERY_FRAME_ZOOM=N` with a matrix
enlarged). Text is drawn a pixel bolder than the font; `--thin` goes back.

## The queue: keep the whole fleet busy (fleet-queue)

`tools/fleet-queue.py` is the newer engine:
- **one queue** that every machine takes from, a job at a time;
- **a keeper** that tops the queue up from the project's planner, restarts
  any lane that dies, and stops adding by itself after five failures in a
  row;
- **two ways to stop.**

treadcore's planner (`scripts/fleet-plan.py`) fills it with two kinds of
work in turn:
- **weight trials:** the scripted brain's 110 weights, a census each,
  chosen by `suggest.py` from the queue's own ledger;
- **net rounds:** continue the best net on calber's GPU, then judge it
  in a census on columbia.

### 1. A queue is a folder

On the Mac, `~/code/treadcore/build/queue/NAME/` holds two files.

`config.json` says who works:

```json
{"hosts": [
  {"name": "calber",   "target": "user@192.168.8.181", "caps": ["cpu", "gpu"], "nice": 10},
  {"name": "columbia", "target": "user@192.168.8.224", "caps": ["cpu"], "nice": 10},
  {"name": "nasser",   "target": "user@192.168.8.121", "caps": ["cpu"], "nice": 19},
  {"name": "mac",      "target": "-",                  "caps": ["cpu"], "nice": 19}],
 "planner": "python3 -I /Users/user/code/treadcore/scripts/fleet-plan.py /Users/user/code/treadcore/build/queue/NAME",
 "low_water": 6, "poll_s": 10, "max_failures_in_a_row": 5}
```

`plan.json` says what to train. The real settings are below; the sample
run used 4 seeds of 120 s.

```json
{"commit": "the treadcore commit every machine builds (pushed)",
 "census_host": "columbia",
 "weights": {"seeds": 55, "secs": 900, "map": "all", "seed0": 6000000,
             "held_seeds": 400, "held_seed0": 7000000},
 "net": {"rounds": [{"run": "rec2400", "lo": 9001260, "n": 1200},
                    {"run": "rec2400", "lo": 9002460, "n": 1200}],
         "train_args": "4 32768 3e-3",
         "census": {"seeds": 110, "seed0": 500000, "secs": 900, "map": "all"}}}
```

### 2. Start it, watch it, stop it

```sh
Q=~/code/treadcore/build/queue/NAME
python3 ~/code/orrery/tools/fleet-queue.py $Q ensure          # start the keeper (in the background)
python3 ~/code/orrery/tools/fleet-queue.py $Q status          # where it is
python3 ~/code/orrery/tools/fleet-queue.py $Q stop-adding     # drain: finish what is queued, collect, end
python3 ~/code/orrery/tools/fleet-queue.py $Q resume-adding
python3 ~/code/orrery/tools/fleet-queue.py $Q stop-now        # stop every job now, collect, end
```

Or from orrery, with the buttons. They appear only for an operator, and
only with `--queue-ctl`:

```sh
cd ~/code/orrery
Q=$HOME/code/treadcore/build/queue/NAME
cargo run --release -- --gui --operator paul --fleet grove --fleet-cmd "$HOME/.local/bin/copal fleet" \
  --training-cmd "cd $HOME/code/treadcore && python3 -I scripts/fleet-view.py --queue $Q" \
  --queue-ctl "python3 $HOME/code/orrery/tools/fleet-queue.py $Q"
```

The Training view then shows:
- the queue's state in its top sentence;
- a bar of done (green), running (amber) and failed (red);
- the counts for each kind of job, the next job, and the last finished;
- what each machine is running.

The buttons are:
- **Stop adding** (or **Resume adding** while draining);
- **Collect results and stop now**;
- **Start the queue**, when its keeper is not running.

### 3. What it found

All of it is under `$Q`:

| File | What it holds |
| --- | --- |
| `plan/summary.json` | written when the queue stops: every weight census and whether it passed, every net round's verdict, the best net |
| `plan/ledger.tsv` | the queue's census ledger |
| `plan/round-NNN.json` | a net round: the candidate less the best, paired by seed, and its held-out agreement |
| `plan/best.bin` | the queue's best net (it changes only past two errors) |
| `logs/ID.log` | each job's commands and output |
| `results/ID/` | what each job brought back |
| `events.jsonl` | every start, finish, failure and stop |

A weight trial that passes is read twice more on 400 held-out seeds. One
that passes both is a candidate: `make families`, then Paul. Nothing
becomes a default, and no net becomes official, without him.

## What it is

| Piece | Where | What it does |
| --- | --- | --- |
| the loop | `tools/fleet-train.sh` (here) | rounds, stages in order, an event a line (`events.jsonl`), STOP, `status --json` |
| the stages | the project's (`treadcore/scripts/fleet-train-stages.sh`) | `stage_record`, `stage_train`, `stage_judge`, and `ft_detail` for the view |
| the recording | `treadcore/scripts/fleet-record.sh` | a queue of jobs claimed by `mv`, a lane a machine, shards streamed to the GPU node and hashed at both ends |
| the lanes as JSON | `treadcore/scripts/fleet-jobs.py` | each machine's job, state, age and the pipe it is running |
| what is shown | `treadcore/scripts/fleet-view.py` | the machines, the run, the loop, the net and its results, as one document |
| the view | `src/training.rs`, the Training face | draws that document; it has no verbs |

## The rules it keeps

1. **The same read rule as every face.** The Training view draws one command's
   document and nothing else; a job that is not in it cannot be drawn.
2. **Watching and steering are two things.** The view has no verbs. The
   loop is started and stopped from a shell.
3. **A failure stops the loop.** A stage that fails ends the round as
   failed; the loop does not retry around it.
4. **A round's best is the loop's, not the project's.** treadcore's judge
   promotes a candidate only if the census, paired by seed on one binary,
   puts it above the best outside two standard errors, and even then the
   official net (`nets/map7.bin`) changes only when Paul says.
5. **Spare means spare.** The hosts file names each machine's threads and
   nice; a machine busy with its own work is left out.
