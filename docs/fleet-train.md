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
