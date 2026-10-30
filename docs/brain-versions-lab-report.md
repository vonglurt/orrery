# Many Versions of One Brain: What the A/B Found, and Whole-Number Choices of Rewritten Functions

*Lab Report — IEEE Format · 30 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: `docs/brains.md` (the map); treadcore `weights.rs` (`Settings::choice`), `commander.rs`
(`Rung::order`), `fleet.rs` (roles, model slots), `scenario.rs` (`set_weights`); orrery
`tools/plans/treadcore.py` (`ab_trials`, `sequential`, `families`, `best_slot`).

---

## Abstract

Paul asked, before anything was published, for a check that the A/B testing and enum matching were
finished as he described them, and then for a report on expanding the method: **several functions
that do the same job, written differently, some reading new senses, chosen by a weight, with only the
chosen one running.**

- **The enum is sound.** `Settings::choice` makes every choice a whole number in range, each of the
  three choices dispatches to exactly one version, and no search can blend them.
- **The A/B was not finished, and the fault was serious.** Every switch's A/B crashed the families
  check, which recorded the crash as a floor broken. **No switch could ever have been promoted.** The
  strongest result in the fleet, pings off at +1.99 a seat on two held-out reads, was being refused
  for it. Mended on both sides.
- **The mended gate then caught a second, worse fault** (§III-B): promoted as it stood, pings off would have
  put a brain on main different from the one measured. Mended, and **pings off is now the default,
  `c015965`.**
- **The method's own harvest is thin so far.** Of the three choices, none has beaten the defaults.
  `ladder = hunt_first` plays *identical* games to the default. All six model slots read lose by about
  20 a seat.
- **There are no rewritten functions yet.** `ladder` reorders, `roles` deals, `model` swaps the whole
  brain. What Paul describes is a new kind of version, and §V sets out how to add it.

---

## I. Objective

To answer two questions from the code and the fleet's own records rather than from the design pages:
is the A/B with whole-number choices finished, and how should it grow?

## II. What Was Asked For, Checked

| Paul's requirement | Where | Finding |
|---|---|---|
| A weight picks a version, 0 as built | `weights!`, a doc saying `(choice: a \| b \| ...)` | **Done** |
| "Whole Number enum only" | `Settings::choice`: `self.w[w].round().max(0.0) as usize`, then the last version at most | **Done:** the version is a `usize` at every use. 1.5 is version 2, never a mix |
| Only the chosen one runs | one `match` or index at each site: `Rung::order`, `MIXES[...]`, `model_of` | **Done** for the three that exist |
| The search must not move an enum as a number | `suggest.py` skips choices; `ab_trials` writes `%d`; `wide_trials` draws from the fit's curves, which hold no choice | **Done,** with one gap: a `tune` or `spsa` run told by hand to vary a choice would still try fractions. Harmless at dispatch, since they round, and wasteful |
| A/B of every version | `ab_trials`: 31 versions on today's defaults, a generation at a time | **Running:** 4 left to ask when read |
| Judged like any weight | the sequential read, two held-out reads, the families | **Broken for switches** (§III) |
| Models as enum 0 to 7 | `model`, slots in `models/models.tsv` | **Census only.** A promoted model would not play in the game or the families (`docs/brains.md` §7) |
| `think_every`, `think_order` | `docs/brains.md` 3b, 3c | **Not built** |
| Alternate functions, new senses | | **Not built** (§V) |

## III. The Fault

pings off was read three times and passed both held-out reads each time it reached them, and was
never promoted. The planner's record said:

```
grove/w1656  caps=baseline, -pings  UNDER  seconds 0  families []
```

A floor broken takes minutes and names the family. This took no time and named none. The job's log:

```
panicked at scenario.rs:1165: not a number: baseline,
test result: FAILED. 0 passed; 1 failed; ... finished in 0.00s
```

`weights_set` wrote the trial as `caps=baseline, -pings`; the test split that on its space and read
`baseline,` as a number. It exits non-zero both for a floor broken and for a panic, and the planner
read both as `UNDER`. Three candidates had it: `-escort`, and pings off twice. Number trials and
choices were never affected: 112 candidates held, and none of them contained a switch.

**Three repairs:**

- `set_weights` reads a `caps` line, sets it on every Commander and leaves a RuleBot alone, and a
  misspelt switch stops the read. Test: `a_switch_is_set_on_every_commander`.
- `weights_set` writes one word.
- **A read with no family in it is `ERROR`, not `UNDER`.** It is read again on the next commit, and
  it does not count as a verdict on the candidate.

The third matters most. The first two mend this crash; the third is why the next one will be seen.

### B. The second fault: a switch promoted but not applied

The first attempt at promotion, r0061, failed a brain test, and the test was right. `set_settings` turned a
switch off only when it saw it *change* from on to off. A dealt bot is made by `new`, then given its role's
fields (`c.pings = true`), then its settings. With pings off in the *baseline* there is no change to see, so the
bot goes on pinging. **Every read of pings off started from an on baseline, where the change was seen. Promoted,
the same brain would have been a different one.** Nothing would have reported it: the next generation would have
measured against a brain still pinging while its weights said otherwise.

Switches now go by state, not by change, and keep what the role gave them
(`a_switch_off_before_the_deal_is_off_after_it`). Two tests assumed pings were on by default and now ask for them by
name, by the rule in `docs/brains.md` §2. The suite passes with pings on and with pings off as the default. The
second attempt, r0062, passed its tests and families on a branch and was pushed.

**No switch had ever been promoted, so nothing had ever walked this path.** The first fault hid the second.

## IV. Results

Each is a pooled read against the defaults in seats a seat; the error is one standard error.

### A. Choices

| Choice | Version | Reads | Result |
|---|---|---|---|
| `ladder` | 1 expand before hunt | 3 commits, 55 seeds each | −0.08, +0.05, −0.23 ±0.3: nothing |
| `ladder` | 2 hunt first | 3 commits, 55 seeds each | **+0.00 ±0.00 each: identical games** |
| `ladder` | 3 expand first | 3 commits, pooled on today's 124 seeds | +0.04 ±0.25: nothing |
| `roles` | 1 no guard | `37b6b65`, 55 seeds | −0.28 ±0.89 |
| `roles` | 2 two guards | `37b6b65`, 55 seeds | −0.10 ±0.76 |
| `roles` | 3 three guards | `37b6b65`, 55 seeds | +0.38 ±0.70 |
| `model` | slots 1 to 6 | 55 seeds each | −22.3, −22.1, −21.0, **−18.9**, −20.6, −20.5 ±~0.5 |

**`ladder = hunt_first` is a dead version, and the cause is not yet known.** Exact equality over 165
seeds on three commits cannot be chance. It does differ in the unit test, so the dispatch works; in
play the reorder never changes a decision. `w_posture` at 0.95 gives the fixed order only a twentieth
of the score, which is a likely part of why all of `ladder` reads near nothing, and it does not by
itself explain exact equality. A census ought to flag a read of exactly 0.00 and stop asking again:
the fleet has spent three reads on it.

**`roles` has only been read on an older brain:** no read on today's defaults yet.

### B. Switches

| Trial | Seeds | Result |
|---|---|---|
| `pings` off | 987 pooled | **+2.03 ±0.19**; held out +2.06 and +1.92 |
| `team_rotate` on | 418 | +1.19 ±0.22, **dropped**: a map under the bar at its last stage, and its last 143 seeds read −0.01 |
| `bridge_jobs` off | 418 | +0.12 ±0.10, dropped |
| `dodge` off | 55 | −6.81 ±0.62: as it should |

**Read that first line carefully before believing it.** A team that stops calling enemies and answering
pings gains two seats. Either the pings are costing more than they give, which is a real finding, or
the score misses what they are for. The census cannot tell these apart, and the families exist to
catch the second.

**What promoting it changed.** With pings off, the team test read "0 pings, 0 seats rallied". As the code reads,
bots no longer answer a call, a person's included. A census of bots rewards this by +2 a seat; **whether a person
playing with bots wants it is not something the census can answer.** It is one line to reverse.

### C. The families check on pings off

With the fault mended, run for real on this Mac: **all nine families hold.** The numbers are identical to
today's defaults, which raised the question of whether the switch was applied at all. So the same path was run
with `dodge` off, and six families fell under their floors (a duel in the open from 79.8% to 40.8%, a rolling
tank under a shell from 99.8% to 89.9%). The switch is applied and the gate is real.

**So the families say nothing about pings:** no family exercises them. They show that dropping pings breaks
nothing else. **The second reading above, that the score misses what pings are for, is not ruled out.** Pings are
for a person's team as well as for bots, and a census of bots would not see what a person loses.

## V. Expanding the Method

### A. The new kind of version

The three choices so far change a *parameter of a decision*. Paul describes something different: the
same decision, implemented again by a different idea.

```rust
// weights.rs
TargetPick "target_pick" 0.0, "tactics: who to shoot (choice: nearest_scored | focus | threat_ranked | lead_time)";

// commander.rs: the one place they differ
let target = match self.settings.choice(W::TargetPick) {
    1 => self.pick_focus(obs),
    2 => self.pick_threat_ranked(obs, &self.senses_threat(obs)),
    3 => self.pick_lead_time(obs, &self.senses_motion(obs)),
    _ => self.pick(obs), // as built
};
```

### B. The rules

1. **A whole number, always.** The choice is read through `Settings::choice` and nowhere else. A
   `match` on it has a `_` arm for version 0. Weights inside a version stay ordinary numbers and are
   tuned as such.
2. **Only the chosen one runs.** The `match` calls one function. **Its senses are computed inside that
   arm**, never ahead of it: a version that is not chosen costs no time and reads nothing. This is also
   what keeps A/B honest: if versions shared a precomputed sense, adding a version could change the
   timing of the others.
3. **Version 0 stays, unchanged.** Written beside, not over. A test pins that 0 is the brain as it was.
4. **Same inputs and outputs at the dispatch.** One return type, so a version cannot reach into another.
5. **Deterministic.** The same seed plays the same game; a census's paired reads depend on it.

### C. More senses for a new function

A sense for a rewritten function differs from a sense for the learned tank (`docs/brains.md` §4), and it
is cheaper to judge: **it needs no ablation, because the version that reads it is itself the experiment.**
Good first candidates, reading what the sim already holds:

| Sense | For |
|---|---|
| time to the next shell landing near the tank | `dodge`, `target_pick` |
| an enemy's closing speed and heading | `target_pick = lead_time` |
| a second enemy contact | `target_pick = threat_ranked` |
| the team board's posture | `ladder`, `waypoint` |
| cover along the way to a goal | `waypoint = covered` |

A sense from outside what a player could see would make the brain cheat. Each must be drawn from what
`obs` already gives that seat.

### D. A/B, A/B/C/D, up to eight

Today each version is compared with the default, alone. That extends to many without new machinery, at
a cost that should be stated:

- **Cost.** About 70 seeds a version a generation. Eight versions of five choices is about 2,800 seeds a
  generation.
- **False passes grow with versions.** Eight tries at a 2% false-pass read give one in seven a false pass
  at the first gate. The held-out reads and the sequential bars absorb this, and they are why it is safe
  to have many.
- **A tournament when two pass.** If versions 2 and 3 both beat 0, the last step should read them against
  each other, on the same seeds, and promote the winner. Today the fleet would promote one, then re-read
  the other on the new defaults, which gets there more slowly. A later improvement, not a prerequisite.
- **Drop dead versions.** Exact zero means stop asking (§IV-A).
- **Interactions are found by promotion.** Each version is read against today's defaults, so a promoted
  version changes what the others are read against. The fleet re-reads on new defaults already, and this
  is why a version dropped once may be worth asking again later.

### E. Order of work

1. ~~Get a clean promotion through for a switch, which proves the mended gate.~~ Done, `c015965`.
2. Find out why `hunt_first` is dead, and flag exact-zero reads.
3. The first rewritten function: **`target_pick`.** It is one decision, already a scorer, cheap to read,
   and `focus_blend` shows that targeting moves results. Two versions and one new sense.
4. Then `waypoint`, and `think_every`/`think_order` as the backlog has them.
5. A guard in `tune` and `spsa`: refuse a choice by name, so a hand-started run cannot waste reads on it.
6. A model in the game and the families, not only the census, before a `model` version can mean anything
   beyond the census.

## VI. Training Changed

Train and school runs now start from the strongest slot read on today's defaults (`best_slot`) unless
told otherwise, which is the league's loop `docs/brains.md` §6 asks for; each generation was starting over
before. **On the last defaults that was slot 4, `g5-r0039-round40`, at −18.9.** Reads on the new defaults start again, and
until a slot is read the newest generation is used. Starting there does not make it good: every slot is far behind
the rules, and nothing yet shows that a warm start closes that gap. The first run with it is the first measurement.
A running school, r0059, keeps the start it had. It has not yet started a run.

## VII. Discussion

The check was worth asking for. Nothing on the design page was false in what it said was built, and yet the
most valuable path through it could not complete. The test that would have shown this, a switch reaching the
families, did not exist, because each part had been tested on its own.

The lesson for §V: **the first rewritten function should go all the way through to a promotion attempt before
a second is written,** so a fault like this one shows up at the cost of one version rather than eight.

### Still open

- Why `ladder = hunt_first` plays identical games.
- `roles` on today's defaults.
- A family of a team answering a call, and a decision on whether bots should answer a person's ping whatever the
  switch says.
- Whether pings off is a real gain or a gap in what the score sees. A family of a team answering a call would
  test it; none exists.
- All of §V is a design. No rewritten function exists.
