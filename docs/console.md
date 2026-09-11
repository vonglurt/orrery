# The museum interface

*The design. What the window contains, what a button does, and what the
interface refuses to offer.*

The wall is eight nodes at a glance and the seat is one node you are working
on. This is the third thing, and it is what the lab report actually described
in §IV-B before either of the other two was built: **a room of machines you
select from, and a row of verbs that act on the selection.** A computer lab,
with the computers in it.

It replaces nothing. `--seat` stays, because one node full-screen in a terminal
is a different job. `--listen` stays, because a phone cannot run this. What
`--gui` becomes is the console the operator stands in front of.

---

## 1 · The object model, and why selection comes first

§IV-A of the lab report puts the object model before the interface, and the
reason is that every argument about buttons is really an argument about what a
button is aimed at. Three objects and one piece of state:

| | |
|---|---|
| **node** | one machine, as `copal fleet state --json` describes it. Never invented here. |
| **selection** | an ordered set of node ids. May be empty, one, or all of them. |
| **verb** | something that happens to the selection, and reports per node. |
| **posture** | gallery or operator. Decides whether the verb bar exists at all. |

The selection is the interface's only mutable state. Everything else on the
screen is a rendering of the last read model, and the read model is still the
CLI's — a node id that is not in the document `copal fleet state` just returned
cannot be selected, because it was never drawn.

**The arity of the selection is a first-class thing, not a detail.** Half the
verbs mean something different on one node than on eight, and two of them mean
nothing at all on eight and are refused. So the verb bar is redrawn every time
the selection changes, and a verb that cannot apply is *absent* rather than
greyed — the same rule the web console already follows for the write route in
the gallery posture, for the same reason.

---

## 2 · The screen

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ orrery   museum                     ● live      8 declared · 7 up · 6 bus│  header
 ├────────────┬─────────────────────────────────────────────────────────────┤
 │ SCENES     │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐        │
 │  show    5 │  │museum-01 │ │museum-02 │ │museum-03 │ │museum-04 │        │
 │  wake    1 │  │● up  44° │ │● up  45° │ │● up  51° │ │● up  46° │        │  the lab
 │  rest    1 │  │show      │ │show      │ │sdr-source│ │show      │        │
 │            │  └──────────┘ └──────────┘ └──────────┘ └──────────┘        │
 │ TAGS       │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐        │
 │  wall    2 │  │museum-05 │ │museum-06 │ │museum-07 │ │museum-08 │        │
 │  sdr     1 │  │● up  47° │ │warden    │ │○ missing │ │◐ silent  │        │
 │  north   3 │  │show      │ │wake  39° │ │08:12     │ │rest  48° │        │
 │            │  └──────────┘ └──────────┘ └──────────┘ └──────────┘        │
 │ STRANGERS  │                                                             │
 │  epson-XY10│  ── seen, never contacted ───────────────────────────────── │
 ├────────────┴─────────────────────────────────────────────────────────────┤
 │ Observe  Control  Terminal  Exchange │ Send  Message  Run  Scene  Power  │  verbs
 ├──────────────────────────────────────────────────────────────────────────┤
 │ 3 selected · museum-01 museum-03 museum-05          operator · cert 7h12m│  status
 └──────────────────────────────────────────────────────────────────────────┘
```

Four regions, and each one is a rectangle `draw.rs` already knows how to fill.
There is no window manager inside the window: no floating panels, no overlapping
anything, no shadow. A pane that opens — a terminal, a screen, a result column —
**replaces the lab**, with the rail and the header and the status line still
there, so there is never a question of what is on top.

### The tile

A tile is the smallest honest summary of a node, and the rule it inherits from
the TUI's wall is that *"not reported" and "45" are different claims*. A node
that never announced carries no temperature, and its tile says so by leaving the
field blank rather than by drawing a zero.

| line | content | absent when |
|---|---|---|
| 1 | the node id, `10x20` | never |
| 2 | glyph + status + temperature | temperature absent when not reported |
| 3 | scene, or role where it is not `node`, or the time last seen | — |
| 4 | a thin bar: tags, and the attention chip | no tags, nothing wrong |

Three pictures, matching the three glyphs `copal-fleet-view` already emits —
`●` up, `◐` announced but the agent is quiet, `○` missing. A fourth state is
drawn but is not a status: **selected**, which is the theme's accent as a 2px
frame, because a selection that is a background colour becomes invisible the
moment a tile also wants to be amber.

**Strangers are a row, not tiles.** §IV-B's rule is that a machine on the LAN
that is not in this fleet is *shown and never contacted*, and the interface
enforces that by making them undrawable as tiles: they have no selection box, so
no verb can ever be aimed at one. The refusal is structural rather than a check.

### The rail

Scenes and tags, with counts, from the `scenes` mapping the read model already
returns. Clicking one **selects** that group — which is how "everything running
the show scene" or "everything tagged north" becomes a selection without typing
anything. This is the whole of the "tree" the lab report's §IV-B describes; a
fleet of eight does not need a hierarchy, it needs two ways to say *those ones*.

---

## 3 · Selecting

| | |
|---|---|
| click | select one, replacing the selection |
| ⌘/ctrl-click | add or remove one |
| shift-click | extend from the last click, in tile order |
| click a scene or tag | select that group |
| ⌘/ctrl-A | all nodes — never strangers |
| Escape | clear |
| ←→↑↓ | move the focus tile; space toggles it |

Keyboard works throughout, because a gallery machine on a lectern may have a
keyboard and no mouse, and because the arrow keys are how this gets tested
without a pointer.

---

## 4 · The verbs

The lab report's table, with the two columns it always had, and a third saying
what actually runs. **Every row is either a command a person could have typed or
a session aimed at one node** — there is no third kind.

| verb | one node | a selection | what it does |
|---|---|---|---|
| **Observe** `o` | view-only screen | the wall *is* observe-many | RDP or RFB, input never sent |
| **Control** `c` | screen, keyboard, pointer | **refused** | RDP where the node is Wayland, RFB where it is X11 |
| **Terminal** `t` | a shell | **refused** | SSH as the login user, `fleet-human` principal |
| **Exchange** `e` | two-pane file browser | **refused** | SFTP over the same connection |
| **Send** `s` | push a file | push to all | SFTP put, per-node result |
| **Message** `m` | banner | banner on all | `copal fleet run message`, once the node has the verb |
| **Run** `r` | one verb | fan-out, a result each | `copal fleet run --node ID …` |
| **Scene** `S` | — | apply | `copal fleet scene --node ID apply NAME` |
| **Snapshot** `k` | restore | restore each | `copal fleet run --node ID snapshot restore` |
| **Power** `p` | off / reboot | the end of the day | `off`/`reboot` over ssh; `on` is the supply's job |
| **Notify** `n` | tell me when this is up | tell me when all are up | `copal fleet notify --all-up` |
| **Media** `M` | — | — | the cards, images and VMs pane — see [media.md](media.md) |

### Control on a multi-selection is refused, and that has not changed

§IV-C is emphatic and `verbs.rs` already encodes it: broadcasting keystrokes to
eight machines produces divergent state nobody can see, and the operation
actually wanted is Scene or Run. The button is not drawn when the selection is
larger than one. Exchange and Terminal go the same way and for a weaker but
sufficient reason — two file browsers or two shells in one pane is not an
interface, it is two interfaces.

### Observe on a selection is the wall

There is no "observe eight" mode to build, because the lab already is one, and
§III-B priced eight live sessions on 512 MB boards and refused them. What
Observe on a selection does is **enlarge**: the selected tiles grow to fill the
lab and the rest are hidden. Still thumbnails, still the read model's `thumb`
field, still one live session at most.

### Terminal is no longer a fleet decision

The README has said, since the seat was built, that *"a shell on a node needs a
second credential with a wider door than the operator certificate has"* and that
the seat therefore says so rather than offering a tab that cannot work.

**That credential already exists, and stage 16 of `copal-prep.sh` already
installs it.** Line 28383 writes `fleet-human` into
`/etc/ssh/principals/$PI_USER`, and `copal fleet login` has always signed the
operator key with `-n fleet-operator,fleet-human`. One certificate, two
principals, two doors: `fleet-operator` lands on the `copal-fleet` account and
is caught by `ForceCommand /usr/bin/copal-fleet-exec`; `fleet-human` lands on
the login account and gets a shell, a tty and a `Match` block that never
mentions it.

So Terminal needs nothing on the node at all. It needs an SSH client in orrery,
which is [wire.md](wire.md)'s subject, and it inherits the certificate's
validity as its ceiling — the status line shows the remaining hours because a
session that will stop working at 17:00 should say so at 09:00.

### Message needs a node-side verb, and does not pretend otherwise

`verbs.rs` currently refuses to offer Message, with a comment that is correct:
`copal fleet notify` means "tell me when all eight are up" and there is no
banner anywhere in the fleet. The button therefore appears only when the node
side has grown one — see [`fleet-control.md`](../../copal-alpine-linux/docs/fleet-control.md) §4.
Until then it is absent, not broken.

---

## 5 · The two postures, carried through

| | | |
|---|---|---|
| **gallery** | default | the lab, the rail, Observe. The verb bar is not drawn. |
| **operator** | `--operator TOKEN`, or an operator key that exists | the verb bar. |

In the web console the distinction is a route that does not exist. In a native
window there is no route, so the equivalent commitment is: **in the gallery
posture the verb dispatch table is empty**, not filtered at the point of use. A
keystroke that would have been a verb finds nothing to call. This matters
because a gallery screen is a machine an unattended member of the public is
standing in front of, and "the button is hidden" is not a security property.

The operator posture additionally requires the operator key to be readable at
`~/.copal/fleets/<fleet>/operator`. Without it the session verbs — Control,
Terminal, Exchange, Send — are absent even under `--operator`, because there is
no credential to make them with. The status line says which of the two is
missing rather than dimming a button with no explanation.

---

## 6 · Results

A verb on eight nodes produces eight answers, and the one thing the interface
must never do is collapse them into a success. The results pane is the lab's
grid with the tiles replaced by result cards — id, exit code, and the first
lines of what the node said — and it stays until dismissed.

*"Six of eight got the memo" is the normal case and the console has to be able
to say so.* That sentence is already in `fleet.rs`; this is where it is drawn.

---

## 7 · What this interface deliberately does not have

- **No node it learned about itself.** The read model is `copal fleet state`
  and nowhere else. Discovery is the CLI's, enrolment is the CLI's, and a
  console that learned a node's address from a beacon would be §12's ordering
  broken.
- **No cache.** Same reason as the web console: a second place the fleet's
  state lives, and a thing that goes stale.
- **No control-all.** §IV-C, above.
- **No settings screen.** Everything configurable is a flag or is in
  `answers.txt`, and a console that writes configuration is a second writer of
  the fleet's state.
- **No window inside the window.** One pane at a time, replacing the lab.

---

## 8 · Open, and recorded here rather than discovered later

- **Observe still has no thumbnails to draw.** `n.thumb` is W5 and the read
  model does not carry one yet. The tile renders the field the moment it
  appears; until then Observe on a selection enlarges tiles that have no
  picture in them, which is honest but thin.
- **The keymap is baked.** See [surface.md](surface.md) §5. A non-US operator
  gets the wrong letters into Terminal and Control until `--keymap` grows more
  than one table.
- **Message is drawn against a verb that does not exist yet**, and the node-side
  half is the larger half.
