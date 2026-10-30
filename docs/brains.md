# More Brains: Versions, Roles, Senses and a League of Models

*Design · 29 October 2026 · orrery and treadcore*

Paul, 29 October: "create more efficient A/B testing alternate functions
such that we can expand binary paths of brains using a or b ... an enum,
perhaps 0-3 for the weights", and then: "make alternate path finding, think
like different units ... choose the order and the processing time to be
adjustable in the weights ... draw additional sensors if its an effective
method ... document adding more brainz, adding ai learning to the functions
... an enum of up to 0-7 different alternate ai models ... name the models
as generations progress."

This page is the map: what is built, how to add a version of the brain,
and the steps toward the rest. Each step says what it needs and how the
fleet will judge it, because the fleet now judges every version the same
way.

---

## 1. What is built

| Part | Where | State |
| --- | --- | --- |
| Weights, about 120 numbers the brain scales by | treadcore `weights.rs`, `weights!` | built; the fleet's search tunes them |
| Capabilities, 18 switches (`kite`, `castle_cap`, …) | `weights.rs`, `caps!` | built; **since `e5b3edd` each is tried against the defaults, flipped, every generation** |
| Choices, a weight that picks a function's version, 0 the brain as it was | `weights.rs`, a doc saying `(choice: a \| b \| …)`; `Settings::choice` | **built, treadcore `c891575`** |
| The first choice, `ladder`: the Auto ladder's order (0 as built, 1 expand before hunt, 2 hunt first, 3 expand first) | `commander.rs`, `Rung::order`, `Rung::pick_in` | **built**; 0 plays as before, bit for bit |
| A/B trials: every switch flipped, every choice at each other version | orrery `treadcore.py`, `ab_trials` | **built, `e5b3edd`**; 21 versions on today's brain, a third of each generation's room at most |
| Promotion of a switch or a choice | treadcore `set-defaults.py` takes a `caps = baseline, …` line; a choice is a number | **built** |
| Undoing a switch a promotion flipped | `anchor_trial` | **built** |
| A learned tank: senses (about a hundred numbers) → a 64×64 net → seven action heads | treadcore `senses.rs`, `net.rs`, `NetBrain` | built; plays a census as the trial team (`CENSUS_NET`) |
| crucible: populations of nets duelling on the GPU | `~/code/crucible` | built; **its own arena, senses and net (H = 32): its champions do not play treadcore** |
| Roles, the choice `roles`: a bots' team of five after its leader (0 as built: a guard, two rangers, a wing; 1 no guard; 2 two guards; 3 three guards) | treadcore `fleet.rs`, `MIXES`, `role_by_rank` | **built, treadcore `b02f1fe`**; a team plays the mix of the settings it is dealt under |
| Model slots, the choice `model`: 0 the rules, k the net in slot k, the trial's and the baseline's alike | treadcore `fleet.rs` census (`CENSUS_MODELS/k.bin`); orrery `models_update`, `model_sends` | **built, treadcore `37b6b65`**; slots 1–7 seeded from treadcore's nets as generation 1 (`g1-bc1` … `g1-phase3`); each finished train or school run is the next generation; health's models check |

A version is judged the way a number is. Its first read is 55 seeds,
then the stages of the read on, then two held-out reads pooled. Its
families and the tests come before a promotion, and the difficulty sets
the bar. Nothing needs to be added to the fleet for a new version: once a
choice is in `weights.rs`, the next generation asks for each of its
versions.

## 2. How to add a version of the brain

1. **Pick the function and keep it.** Version 0 is the code as it is. Write
   B beside it, not over it.
2. **Name the choice in `weights!`.** The default is 0, and the doc names
   the versions:
   ```rust
   Ladder "ladder" 0.0, "auto: the ladder's order (choice: as_built | expand_before_hunt | hunt_first | expand_first): ...";
   ```
3. **Dispatch on `self.settings.choice(W::Name)`** at the one place the
   versions differ. A `match` with `_ => version 0` keeps out-of-range
   values safe.
4. **Test that 0 is the brain as it was, and that each version is what it
   says.** `each_ladder_puts_the_base_first_and_reorders_the_middle` is the
   pattern. If an existing test pins a number the choice moves, give that
   test the number it was written for. It should test its rule, not the
   defaults (`9032026`, `5ef8ad7`).
5. **Commit and push.** The fleet builds the commit, and the next
   generation asks for every version (`ab-NAME-VERSION`). A version that
   passes is promoted as `NAME = k`.

A choice should have at most eight versions, 0–7. Each costs about 70
seeds a generation (55 read, plus a read-on now and then). The 21 versions
now on the brain cost about 1,500 seeds a generation, under a third of an
hour of the fleet.

## 3. Units that think differently

**What the research says.** Games give units different movement in three
ways:
- **Clearance:** each place on the map records the largest unit that fits
  there, so one graph serves every size [1], [2].
- **Hierarchy:** plan on a coarse map, then refine, to bound the cost of a
  long search [3].
- **Time-slicing:** a budget a frame, the important requests first, the
  rest less often [4]. Threads cost determinism there, and a census needs
  determinism.

Unit control in RTS research picks a strategy for each group from a small
set, rather than one policy for every unit [5]. That is a choice, as
built here.

**What fits treadcore.** The world plans every route (the sim's
`Map::route_by`, eight ways on tiles, priced by terrain, pillboxes' reach
and mines). The brain chooses *where* to go and *what* to do. So the
brain's versions belong in its decisions:

| Step | Choice or weight | What changes | Needs |
| --- | --- | --- | --- |
| 3a | `role` (choice: as_built \| hunter \| builder \| scout) per seat | the ladder's order and the switches a seat runs: a hunter's ladder is hunt first and kites; a builder fortifies and lines roads; a scout expands and calls | `Rung::order` (built), a seat index in `Settings`, a census that seats roles (fleet.rs) |
| 3b | `think_every` (rounded, ticks) | how often the Commander thinks; the ladder and target pick run on it | one constant (`MOVE_EVERY`) made a weight; the census records seat-seconds a think, so a cheaper brain shows its cost |
| 3c | `think_order` (choice) | which of the Commander's parts run first in a think (dodge, target, ladder, posts) when the budget is short | 3b first |
| 3d | `waypoint` (choice: direct \| covered \| roads) | where a Move points: straight at the goal, via cover (forest), or via roads; the world still routes each leg | `Map` terrain reads, already in the brain |
| 3e | a danger price the brain hands the world | the sim already routes with a danger function (`route_with`); a brain-supplied price (its own reading of guns and shells) | a new Command field: a change to the protocol, so last |

Each row is a choice the fleet will A/B on its own once it is built. 3a
gives the most: one choice changes four parts at once, and the census
already plays three teams.

## 4. Senses

The learned tank feels about a hundred numbers (`senses.rs`): eight
sectors of eleven channels, its body, its instruments, four periscope
contacts, the scripted brain's hints, the shell and the air. Each is a bar
on the Net page.

**More senses help only if the net uses them.** The effective method is
ablation:
1. Zero one channel.
2. Census the net against itself unchanged, on the same seeds.
3. A channel whose zeroing costs nothing is not being used.

Run this before adding any sense. A sense the net does not use costs
training time and tells you nothing. Senses worth adding next, if
ablation shows the net reads the near ones:
- a second enemy contact;
- the team board's posture as a sense, not only a hint;
- the time to the next shell landing near the tank.

The Net page should show each sense's ablation cost beside its bar. That
is the drawing that earns its place.

## 5. Learning inside a function

A hand-written scorer, such as the target pick (`Tactician::pick`) or a
rung's `holds`, can gain a learned version as a choice:
`target (choice: rules | learned)`.

1. Version 1 is a small net from that function's inputs to its output.
2. Train it first to copy version 0 (behaviour cloning on a census's own
   games), so it starts where the rules are.
3. Then train it by evolution on census results. crucible's loop does
   exactly this on the GPU.
4. The fleet A/Bs it like any version. It is promoted only if it beats the
   rules on held-out seeds.

This learns one decision at a time, with everything else held as built.
It needs far less data than a whole learned tank, and a failure costs only
that choice.

## 6. A league of models, 0–7

The pattern is AlphaStar's league [6]–[8]:
- main agents train against frozen copies of every past generation;
- exploiters train to beat the main agents, so their weaknesses are found;
- a known limit: late in training the exploiters stop finding weaknesses,
  and diversity narrows [7], [8]. Goal-conditioned exploiters are the
  published remedy [7].

For treadcore:

| Slot | What | Named |
| --- | --- | --- |
| 0 | the rules (no net) | `rules` |
| 1 | the main model, trained against all frozen slots | `main-gN`, N its generation |
| 2–5 | frozen past generations of the main model | `gN-<date>`, kept while they still beat something |
| 6 | an exploiter, trained only against slot 1 | `exploit-gN` |
| 7 | the newest crucible champion, distilled into a treadcore net | `crucible-<island>-<segment>` |

**Steps:**

1. **Model slots in the census.** `CENSUS_MODEL=k` loads `nets/models/k.bin`
   for the trial team (fleet.rs already loads `CENSUS_NET`). The slot table
   `nets/models.tsv` holds a slot, a name, a generation and a file hash.
   orrery sends the files with each prepare job: nets are not in git, which
   is why `nets/bc1.bin` and `nets/bc2.bin` stay untracked.
2. **A model choice:** `model (choice: rules | slot1 | … | slot7)`, as a
   weight, so the fleet A/Bs each slot against the rules on the same
   footing as everything else. A slot that beats the rules is promoted
   like a weight. The names live in `models.tsv`, not in the doc, because
   they change each generation.
3. **Generations.** When the main model is trained again, its old weights
   move to the oldest frozen slot (2–5), and the slot whose census against
   slot 1 is weakest is retired. The names record the move.
4. **Feeding crucible back.** Its champion duels in its own arena with its
   own senses. A bridge plays it as a teacher inside treadcore's crucible-
   style arena and clones it into a treadcore `Net` (behaviour cloning, as
   `bc1` was made). The result goes in slot 7, and the fleet reads it like
   any slot. Without the bridge, crucible's champions can only be compared
   with each other.
5. **Training against the league.** The trainer's opponents are drawn
   from slots 0–5 by how often each beats the model in training
   (prioritized fictitious self-play, AlphaStar's PFSP [6]). The
   exploiter's opponent is slot 1 alone.

Steps 1 and 2 make every model a version like the rest. Steps 3–5 are
where the learning happens. Do them in order: a slot the fleet cannot
read is a model no one can judge.

## 7. Order of work

1. ~~Choices, the ladder, A/B for every switch and choice~~ (built).
2. ~~`roles` (3a)~~ (built, `b02f1fe`). One choice for the team's mix;
   per-role ladders and switches are the next step of it.
3. ~~Model slots and the `model` choice (6.1–6.2)~~ (built, `37b6b65`).
   Still open: a promoted model plays in the census only. The game's bots
   and the families gauntlet still deal the rules, so a model's families
   "hold" without testing it. Slot 1 (`bc2` then) read −10.3 a seat
   against the rules over 2 seeds: the first generation is far behind, as
   expected.
4. Sense ablation on the Net page (4).
5. `think_every` and `think_order` (3b, 3c).
6. A learned target pick (5).
7. The crucible bridge and the league (6.3–6.5).
8. `waypoint` (3d), then the brain's danger price (3e).

## References

[1] D. Harabor and A. Botea, "Hierarchical path planning for multi-size
agents in heterogeneous environments," and "Clearance-based pathfinding
and hierarchical annotated A* search," ANU,
https://researchportalplus.anu.edu.au/en/publications/clearance-based-pathfinding-and-hierarchical-annotated-a-search/

[2] GameDev.net, "A*: which cell size," discussion of a per-location
maximum radius, https://gamedev.net/forums/topic/611357-a-which-cell-size

[3] University of Huddersfield, "A hierarchical task network planner for
pathfinding in real time strategy games,"
https://pure.hud.ac.uk/en/publications/a-hierarchical-task-network-planner-for-pathfinding-in-real-time-/

[4] GameDev.net, "Time slicing or multithreading for pathfinding,"
https://gamedev.net/forums/topic/627306-time-slicing-or-multithreading-for-pathfinding/

[5] "Stratified Strategy Selection for Unit Control in Real-Time Strategy
Games," IJCAI 2017, https://www.ijcai.org/proceedings/2017/522

[6] DeepMind, "AlphaStar: Grandmaster level in StarCraft II using
multi-agent reinforcement learning," 30 October 2019,
https://deepmind.google/discover/blog/alphastar-grandmaster-level-in-starcraft-ii-using-multi-agent-reinforcement-learning/

[7] "Revisiting the league training of AlphaStar (ROA-Star)," NeurIPS
2023, https://papers.neurips.cc/paper_files/paper/2023/file/94796017d01c5a171bdac520c199d9ed-Paper-Conference.pdf

[8] "TStarBot-X: An open-sourced and comprehensive study for efficient
league training in StarCraft II full game," https://arxiv.org/pdf/2011.13729v1
