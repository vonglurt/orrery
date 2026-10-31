# Too Many Shots: Ammunition, Walls and Castles as Balance Levers in a Tank Game

*Paul Richeson · Lab Report — IEEE Format · 30 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery.

Companions: treadcore `source/treadcore-sim/src/rules.rs`, `source/treadcore-brain/src/front.rs`,
`source/treadcore-brain/src/ledger.rs`; orrery `tools/plans/treadcore.py`.

---

## Abstract

An operator watching bot matches of a tank game reported too many shots being fired. We changed the game's supply, walls and
scoring in response, and measured the result. Depots now make one item a second, shared, in place of an unlimited
two shells and two drones a second per tank. Walls gain a second purpose: a tank inside a castle takes half damage from
drones. Turrets past two in one castle slow all of that castle's guns, and new rewards pay for whole supply trips, rotating
between bases, and standing castles. Over eight seeds of one map, shots fell from 5.53 to 4.72 per tank-minute, a difference
of 1.9 standard errors, which is just below our bar. Strike drone hits rose from 63 to 95 a match. We therefore have not solved
the reported problem. Nearly all shots are indirect, and our arithmetic suggests pillboxes can now supply more shells
than depots. We review how other games use walls and occlusion, and we state what to test next. No census of these changes has finished.

**Index Terms:** game balance, line of sight, real-time strategy, resource economy, reward shaping, simulation.

---

## I. Introduction

A game's balance is partly a question of how much shooting it allows. If shots are free, position and timing stop mattering,
and if they are too scarce, a player spends the match driving to supply and not fighting. The operator asked for "a balance
play" that still allows shooting and stays fun.

The operator named six levers: less ammunition; rewards for leaving a base full and returning low; rotation between bases;
a sensor for base ammunition, distance and direction; a soft limit on turrets and bases past two per castle; and walls that stop direct
shots and reduce drone damage. This report describes each, what it does, and what we measured.

## II. How Other Games Treat Walls and Occlusion

We searched for how comparable games use walls, cover and supply. Sources vary in quality, and several are community
wikis or forum posts, so we mark them. We did not play or test these games ourselves.

**Table I. Walls and occlusion in other games.**

| Game | Rule | Lesson | Source quality |
|---|---|---|---|
| Bolo (1987) | Walls and trees block bullets; a tank enclosed in forest cannot be seen; trees are the builder's money | The direct ancestor of our rules | Project documents [1], [2] |
| Logo Bolo | Bases hold limited armour and ammunition that regenerate slowly | A precedent for a shared depot store | A re-creation's README [2] |
| Red Alert | Concrete walls block direct fire such as tank cannon; artillery and aircraft are unaffected | Walls that stop one class of weapon make weapon choice matter | Search summary of a community wiki [3] |
| Red Alert 2 | Some tanks fire over walls | Exceptions can carry a unit's identity | As above [3] |
| Company of Heroes 2 | Heavy cover reported as -50% damage, by direction; accuracy penalties in cover | A visible, large, simple bonus | Forum report and patch note [4]; no official source found |
| StarCraft: Brood War | About a 53% hit chance from low ground or under trees; sources disagree on the exact figure | Occlusion as a chance, not a wall | Community wikis [5], [6] |
| Age of Empires II | Palisades slow enemies and give early warning | Walls buy time and warning, not only protection | Community wiki [7] |

Three patterns recur. Walls differ by weapon class, so a wall alone is not a complete answer. Cover works best when it is easy to see. And
walls often buy time as much as they reduce damage. For supply, a Cornell lecture frames resources as sources and sinks and warns that
underpriced actions get overused [8], which matches what we see. Deus Ex: Human Revolution shows the risk on the other side: cutting ammunition drew complaints
of scarcity [9].

For rewards, Ng, Harada and Russell show that shaping of the form γΦ(s′) − Φ(s) leaves optimal policies unchanged, and they
discuss agents looping to collect a bonus [10]. Our new rewards are not of that form. They change what the search prefers, on purpose, and so they can be farmed.

## III. The Changes

**Table II. What changed (game version 3).**

| Lever | Before | Now |
|---|---|---|
| Depot supply | 2 shells/s and 2 drones/s for each tank, unlimited | 1 item/s in all, round 6 kinds; store of 20 each, shared; starts empty |
| Pillboxes | Hold 8, regrow 1 per 10 s | Unchanged, at the operator's choice |
| Trees | Never regrew | Regrow after 2 minutes |
| Tank in a castle | No protection from drones | Half drone damage, with 8 walls within 6 tiles |
| Turrets past 2 within 12 tiles | Full rate | Each extra adds half a reload to all |
| `castle_inside` default | 6.53 | 2 |

**Walls.** A wall already blocked flat shots and pillbox fire through line of sight. We left indirect shells unchanged, following Red
Alert's distinction, so walls do not make a castle immune. The 50% share is our starting figure, taken from the reported Company of Heroes 2 value, and
no test has tuned it.

**The soft limit.** We read "castles of turrets and bases more than 2" as discouraging more than two guns in one castle. Nothing refuses a build.
Depots are fixed by the map and are not limited in play. Held score no longer counts guns past two in a group.
Depot holdings still count in full, so the limit on bases is weaker than the limit on turrets.

**Rewards.** Each is added to a seat's score, and so shapes what the fleet's search selects.

**Table III. New rewards.**

| Reward | Paid for | Weight | Guard against farming |
|---|---|---:|---|
| Full sortie | Leaving a base at 90% shells or more | 0.5 | Needs a shot fired since the last sortie |
| Low return | Reaching a base at 25% shells or less | 0.5 | Needs shells to be spent |
| Rotation | Reaching a base other than the last | 0.75 | That base must hold at least 5 shells |
| Castle | Each castle standing, over match length | 3.0 | Needs a gun among 8 walls; extra guns add nothing |
| Enemy base entry | Each visit | 0.5 | Once a visit; no guard against repeat visits |
| Raid | Mine laid past the battle line, then home alive | 1.5 | Needs the return |
| Mine hit | Hostile tank caught | 1.0 | None needed |

These weights are starting guesses. A tank could still earn the low return by firing wildly, because the reward does not ask whether its
shots hit. The existing direct-hit reward does not cover mortars, which are nearly all the shots.

**The sensor.** Four new inputs give the best base's shells, distance and bearing, where "best" means most shells for the road. The transform now
takes 153 inputs. The trained nets keep their 143, because every net on file was made for that shape, so they do not see this sensor. The rule brain uses it too:
`home_pick` sends a tank to the best base in place of the nearest, and it is on by default.

**Battle lines and mines.** A line is where the nearest ground of ours is as far as the nearest of theirs. The choice `mine_plan` selects mines
on the approach, across the line, or a raid into an enemy base that lays one and runs. It stays off until a census reads it.

## IV. Results

We counted events over eight 8-minute matches of 16 tanks on one map (`naval`), on the same seeds, before the ammunition change (commit e34d762) and with every change above.

**Table IV. Shooting per match (means over 8 seeds).**

| Measure | Old game | Game 3 | Change |
|---|---:|---:|---:|
| Shots per tank-minute | 5.53 | 4.72 | -15% |
| Direct shots | 7.6 | 4.8 | -37% |
| Indirect shots | 700 | 600 | -14% |
| Shells that hit something | 190 | 158 | -17% |
| Strike drone hits | 63 | 95 | +52% |
| Tanks destroyed | 29 | 35 | +22% |

The drop in shots is -0.80 per tank-minute, with a paired standard error of 0.42 (1.9 standard errors), just under the project's bar of two, so we read it as likely but not established. The rise in strike hits is 32 with a standard error of 13 (2.5 standard errors), past the bar. The rise in tanks destroyed is 1.7 standard errors. (The commit message of 71f3578 gives 4.85 and 56, from arithmetic done by hand; the figures here are computed from the raw counts.)
A match is chaotic: one test's result swung from 0 to 8 seats answering a ping as the tree regrowth moved between 100 s and 180 s. Eight seeds of one map can only give a rough figure.

Three things stand out:

1. **Almost every shot is a mortar.** About 99% of shots are indirect. Walls stop direct fire, so walls do almost nothing about what the operator sees.
2. **Drones rose.** Strike hits increased by about half. We have not established why. Shots and strikes may be substitutes, and the kill count rose too, so the game may have grown more lethal, not less.
3. **Pillboxes may now be the larger source of shells.** With 20 pillboxes owned, regrowth could supply up to 2 shells a second across the map. Ten depots at one item a second, round six kinds, would make about 1.7. We did not count which source actually fed the shots, and refineries also supply shells.

## V. Discussion: Allowing Shooting and Keeping It Fun

The aim is not fewer shots for its own sake. A shot is fun when it was a choice, when it can hit, and when missing costs something. At 5 shots
a tank-minute, each tank fires once in about 12 seconds, and 16 tanks together fire over a shot a second. With about one shell in four hitting, the air is full of misses.

Ammunition is a weak lever here because tanks start with 24 shells, and a mortar shot is cheap. Cutting supply further risks the Deus Ex result, in which players simply feel starved. Three options address the shot itself, and we would test them in this order:

1. **Price the mortar.** A longer reload or two shells a shot targets the 99% directly and leaves direct fire free.
2. **Reward hits, not firing.** Extend the hit reward to indirect shells, and pay a small cost for a miss. The low-return reward then cannot be earned by firing wildly.
3. **Count pillbox shells.** Measure each source's share before cutting anything. The operator kept pillboxes as they are, so this needs their decision.

Walls may also need more. If mortars dominate, a castle's protection against drones matters less than whether anything protects against shells. Red Alert's answer is that nothing does, and the counter is to go out and kill the artillery. That suits a game about leaving the base.

## VI. Limits

- **No census.** The fleet began reading game 3 only today. No reward weight, wall share or crowding figure has been tested.
- **One map, eight seeds, bots only.** No person has played this version, so we have no evidence yet on whether it is fun.
- **All changes measured together.** Table IV cannot say which change did what.
- **Workers not seen.** Base workers compile, but we have not seen them on screen.
- **Thin sources.** Several game rules come from wikis and forums, and we did not verify them in play.

## VII. Conclusion

The depot change delivered the supply cut the operator asked for, but it did not clearly reduce shots, and drone hits rose. The shots are almost all mortars, so
the next lever should be the mortar's price, not more ammunition cuts. To decide, we will count shots by source and run a census of a slower mortar, confirming it only if shots fall by more than two standard errors, with hits per shot rising, and with no family falling under its floor.

## Acknowledgment

Paul Richeson set the direction and chose the rules. Claude (Anthropic) wrote the code, ran the measurements and drafted this report.

## References

[1] "Bolo FAQ and MacBolo instructions," archived documentation. https://websites.umich.edu/~archive/mac/game/war/bolo/misc/recgamesbolofaq1.9.txt

[2] B. Kurtovic, "Logo Bolo: a re-envisioning of the classic tank game by Stuart Cheshire in NetLogo." https://git.benkurtovic.com/ben/lobo

[3] Command & Conquer Wiki, "Wall." https://cnc.fandom.com/wiki/Wall_(Tiberium_Alliances)

[4] Company of Heroes 2 community pages and forums. https://www.coh2.org/guides/5732/company-of-heroes-2-basic-concepts-and-glossary

[5] Liquipedia, "Terrain Features," StarCraft. https://liquipedia.net/starcraft/Terrain_Features

[6] StarCraft AI Wiki, "Chance to Hit." https://www.starcraftai.com/wiki/Chance_to_Hit

[7] Liquipedia, "Palisade Wall," Age of Empires. https://liquipedia.net/ageofempires/Palisade_Wall

[8] Cornell University CS 3152, "Balance," lecture slides, 2013. https://cs.cornell.edu/courses/cs3152/2013sp/lectures/07-Balance.pdf

[9] Game Informer, "Why is ammo so scarce in Deus Ex: Human Revolution," 2011. https://www.gameinformer.com/b/news/archive/2011/08/17/why-is-ammo-so-scarce-in-deus-ex-human-revolution.aspx

[10] A. Y. Ng, D. Harada and S. Russell, "Policy invariance under reward transformations: Theory and application to reward shaping," in *Proc. Int. Conf. Machine Learning*, 1999. https://mlanthology.org/icml/1999/ng1999icml-policy/
