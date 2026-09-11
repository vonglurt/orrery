# Cards, images and machines

*The install-media pane: what it drives, what it refuses to do itself, and the
manifest that turns eight cards from something remembered into something
written down.*

---

## 1 · orrery does not write a card

`copal-prep.sh` picks the disk and asks for both typed `ERASE` confirmations,
and — in `bin/sd.sh`'s own words — *that is the entire safety model for this
path.*

The pane is a front end to it and nothing more. It shows what is about to
happen, runs `make`, streams the output into a `term` widget, and shows what
happened. **The confirmations still happen, inside the pane, typed by the
operator, read by `copal-prep.sh`.** Nothing is answered on their behalf and
there is no `--yes`.

The reason is not caution for its own sake. `copal-prep.sh` is 1.6 MB of shell
that knows nine boards across four architectures, the boot-partition label
collision that makes two builds unsafe at once, the mount-point refusal, and
which `/dev/rdiskN` on a Mac is a card rather than the startup disk. A second
implementation in Rust would be a second thing that can be wrong about which
disk is which, and the failure mode of being wrong about that is somebody's
photographs.

---

## 2 · What the pane actually shows

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ Media · fleet museum · 8 cards                                      [×]  │
 ├──────────────────────────────────────────────────────────────────────────┤
 │  #  hostname      role     tags        token     built           node    │
 │  1  museum-01     warden   wall        spent     2026-09-04 sd   ● up    │
 │  2  museum-02     node     wall        spent     2026-09-04 sd   ● up    │
 │  3  museum-03     node     sdr         spent     2026-09-05 sd   ● up    │
 │  4  museum-04     node     wall        spent     2026-09-05 sd   ● up    │
 │  5  museum-05     node     north       spent     2026-09-05 sd   ● up    │
 │  6  museum-06     node     north       spent     2026-09-06 sd   ● up    │
 │  7  museum-07     node     north       spent     2026-09-06 sd   ○ miss  │
 │▸ 8  museum-08     node     —           UNUSED    —                —      │
 ├──────────────────────────────────────────────────────────────────────────┤
 │ board [ zero2 ▾ ]   answers.txt says: card 8, museum-08, role node       │
 │                                                                          │
 │ [ Write card ]  [ Build image ]  [ Make VM ]      [ Prepare card 8 ]     │
 ├──────────────────────────────────────────────────────────────────────────┤
 │ ~/.copal/fleets/museum/media · 9 entries · last 2026-09-06 14:02         │
 └──────────────────────────────────────────────────────────────────────────┘
```

Four sources, none of them invented here:

| column | read from |
|---|---|
| # , hostname, role, tags | `answers.txt` for the current card; `~/.copal/fleets/<f>/tokens` for the rest |
| token | the ledger's third field — `unused` or `spent` |
| built | the media manifest, §5 |
| node | the read model — has this card's hostname ever announced itself? |

That last column is the one that makes the pane worth having. **A card, a token
and a running machine are three different things**, and the question the
operator actually has at 08:45 is "which of these eight did I not finish", which
needs all three in one row.

---

## 3 · The sequence, enforced instead of remembered

Today the Makefile documents a discipline:

```
make answers            once, naming the fleet
make image MODEL=zero2  card 1
make answers-node N=2
make image MODEL=zero2  card 2   ... and so on
```

and the comment above `answers-node` says why it must be that way: *"Build the
card between each one -- the answers file describes whichever card is next."*

That is a stateful sequence with no memory of where it is. The operator counts
cards in their head, and the failure is silent: run `answers-node N=5` twice and
card 4 is never written, or forget to run it at all and two cards are both
`museum-03` — two machines with the same identity and the same enrolment token,
which is an hour of confusion at the far end.

The pane makes each step a check:

- **Prepare card N** runs `tools/copal-answers.sh --node N` and is only offered
  for a row whose token is not already `unused` — because re-preparing a card
  that has not been written supersedes a live token for no reason, and the
  ledger's own comment says why that is bad: *"two live tokens for one machine
  would mean the check has two right answers, which is not a check."*
- **Write card** is only offered when `answers.txt`'s `COPAL_FLEET_INDEX`
  matches the selected row. The pane will not write card 8 while the answers
  file describes card 3; it says which card the file describes and offers to
  prepare the right one.
- After a successful write, the row's *built* column fills in from the manifest
  and the selection advances to the next unbuilt card. The next thing to do is
  the next thing highlighted.

**Nothing is bypassed and no new authority is created.** Every button is a
`make` target that already exists, run in the order the Makefile already
documents. What is added is that forgetting a step is now visible.

---

## 4 · The three targets

| button | runs | writes |
|---|---|---|
| Write card | `make sd-$BOARD` | a physical card; `copal-prep.sh` picks the disk and asks twice |
| Build image | `make img-$BOARD` | `build/copal-$BOARD.img`, and now `build/copal-$BOARD.img.sha256` |
| Make VM | `make vm MODEL=$BOARD` / `make utm-x86` | a UTM machine from the same image |

One answers file behind all three, so a fleet can be eight cards, or six cards
and two VMs, or eight images written to cards later on a different machine — and
the manifest records which it was.

---

## 5 · The manifest, which is the secure part

The ask was *a secure way to orchestrate the collection of install media.* The
insecurity in a pile of eight SD cards is not cryptographic. It is that a card
is an anonymous object with a one-time enrolment token baked onto it, and
nothing on the outside of it says which one it is.

So each write appends one line to `~/.copal/fleets/<fleet>/media`, mode 600,
tab-separated, append-only:

```
2026-09-06T14:02:11  8  museum-08  zero2  sd    -                      a3f10c2e  ok
2026-09-05T09:41:02  5  museum-05  zero2  img   9f2c…(sha256 of .img)  7be4419d  ok
2026-09-04T16:20:55  1  museum-01  zero2  sd    -                      1d0ea77b  ok
```

| field | and why |
|---|---|
| timestamp | when this physical object came into existence |
| index, hostname | which card it is |
| board, target | what it will boot, and whether it is a card, an image or a VM |
| sha256 | of the `.img`, where there is a file. A card written later from that image is provably that image. |
| token fingerprint | **the first 8 hex of SHA-256 of the token — never the token.** Enough to match a card to a ledger row; useless to anyone who reads the file. |
| outcome | `ok`, or the exit status, because a failed write is a fact about a card too |

What that buys, concretely:

- **"Which card went missing?"** — the ledger says which token is still
  `unused`, the manifest says when that card was written, and `copal fleet ls`
  says nothing has ever enrolled with it. One row, three sources agreeing.
- **"Is this image the one we tested?"** — the `.sha256` beside it.
- **"Did card 6 ever get written?"** — the manifest, rather than memory.
- **A token that appears on two cards is visible**, because two manifest rows
  carry the same fingerprint.

### And what is deliberately not in it

- **Not the token.** A fingerprint matches; it does not enrol.
- **Not the password hash**, which is in `answers.txt` and stays there.
- **Not the CA private key**, which lives at `~/.copal/ca/<fleet>_ca` and which
  nothing in orrery reads, writes, copies or mentions. The console holds the
  operator key; the CA is the thing that signs operator keys, and a program that
  held both would be a program worth stealing.
- **Not in git.** `~/.copal` is outside every checkout, mode 700, as
  `settle()` already enforces.

---

## 6 · Where it runs, and the one thing that follows from that

This pane is **the reason `src/mac.rs` exists.** `answers.txt`, the Makefile,
`copal-prep.sh`, the CA and the token ledger all live on the operator's Mac.
A Wayland-only console could show a fleet and could not make one.

On a Linux node the pane still opens, and every button still works if that node
has the checkout — `copal-build` compiles orrery on nodes already, so this is
not hypothetical — but the common case is a Mac with a card reader.

What the pane does **not** do is write a card on a remote machine. There is no
"write card on the warden" button. Media is made where the operator is standing,
because the confirmation is a person looking at a disk.

---

## 7 · What `copal-alpine-linux` has to grow for this

Small, and all of it is described in
[`fleet-control.md`](../../copal-alpine-linux/docs/fleet-control.md) §7:

| | |
|---|---|
| `make answers-show --json` | so the pane reads the answers file through the tool that owns it rather than parsing shell assignments |
| `img-%` writes a `.sha256` | so the manifest has something to record |
| `make vm MODEL=` | the VM path, parameterised the way `sd-%` and `img-%` are |
| `tools/copal-media.sh` | append a manifest row; called by the Makefile after a successful write, so a card written from the command line is recorded too |

That last one matters more than it looks. **The manifest must not be something
only the GUI writes**, or it becomes wrong the first time somebody types
`make sd-zero2`, and a ledger that is sometimes right is worse than none.
