# orrery

The Copal fleet's web console — the wall, the seat, and the verbs, served by
the warden.

An orrery is a clockwork model that shows many bodies at once and lets you turn
a handle to see where they will be. That is the wall.

## What it is

§12 of `copal-alpine-linux/docs/fleet-plan.md` names three faces on one read
model, and gives this one its job: *"the gallery screen, and the phone in the
operator's pocket — third, served by the warden, read-mostly."* This is that
face, and it is also the only place the Timbuktu verbs surveyed in
`docs/fleet-lab-report.md` can land — Control, Observe, Exchange, Send and
Message are still dimmed in the TUI's footer with the words `L6, not built`.

It replaces the Python prototype at `copal-alpine-linux/tools/copal-fleet-web`,
which stays as the reference for what the page should do.

## The rule it inherits

`copal-fleet-console.py`'s acceptance test was architectural: it opens no
socket, holds no credential, knows no subject names, and every screen is a
rendering of a command a person could have typed. This keeps that test. The
whole backend is `Command::new("copal").args(["fleet", …])`. A console that
learns to talk to nodes directly has become a second way of knowing things.

## Two postures

Read-mostly is a default, not a ceiling. A gallery screen and a phone on a
lanyard are two audiences with different rights.

| | | |
|---|---|---|
| **gallery** | default | State, thumbnails, Observe. No verb that changes anything. |
| **operator** | `--operator TOKEN` | The write verbs as well. |

Without a token the write route **does not exist** rather than answering 403 —
a route that says "forbidden" tells a scanner it is there.

## No dependencies, and that is the design

`copal-build` compiles the checkouts in `~/code` on the node itself, and a
fleet node is on a LAN that invariant 6 says never reaches the internet. A
dependency is therefore a crate fetch that fails on the one machine this is
meant to run on. The HTTP needed here is a subset small enough to write —
two routes, one content type, no keep-alive, no TLS, no uploads — and every
absence is a thing that cannot then be got wrong.

Measured on `saskatchewan` (Alpine 3.24, aarch64, cargo 1.96.1):

| | |
|---|---|
| Clean release build, offline | 4.5 s |
| Binary | 453 KB |
| Resident, serving | **1.1 MB** — against 22 MB for the Python prototype |
| Tests | 80 passing |

That last row is the argument on a Zero 2 with 512 MB that is also being the
exhibit.

The first three rows predate the seat and have not been re-measured on
`saskatchewan`. Locally the binary is 471 KB with `rfb`, `paint` and `seat` in
it; the seat's own footprint is one framebuffer, which is `w * h * 3` bytes
held twice — about 1.5 MB for a 720p node, and it is only paid while a seat is
open.

## The seat

The wall is eight nodes at a glance. The seat is one node you are working on,
full screen, in the terminal you are already in — and it is where the Timbuktu
verbs land.

```sh
./target/release/orrery --seat museum-01
```

§III-B of the lab report is the design this follows, and the sentence that
matters is the asymmetry: *screenshots for the seven, VNC for the one being
controlled.* Eight live sessions were priced and refused; one, aimed at the
node in front of you, is the shape that fits the hardware.

| | | |
|---|---|---|
| **Observe** | `o` | the node's screen, read-only |
| **Control** | `c` | keyboard and pointer as well; `Ctrl-]` lets go |
| **Exchange** | `e` | not built — the pane says so |
| **Terminal** | `t` | not buildable as the fleet stands — the pane says why |

It renders with half-blocks (`U+2580` and 24-bit colour) anywhere, and with
sixel where the terminal has it — `--sixel` forces it, `--half-block` forces
the portable path. `--fps` asks the node for a frame rate; the default is 6,
because §III-B's finding is that frame rate is the expensive axis.

**What the seat needs on the node.** `x11vnc -nopw` on the node being
controlled. Two absences are deliberate: the seat speaks RFB security type
`none` only — VNC's own authentication is 56-bit DES and an 8-character
password, which is not security, and the fleet's gate is the certificate and
invariant 6's LAN — and it asks for Raw and CopyRect only, because every
compressed encoding wants zlib and a LAN carrying one session does not need it.

**The read model is still the CLI's.** The address the seat dials came out of
`copal fleet state` and nowhere else; a node id that is not in that document is
refused before a socket is opened. §12's rule is about how the console *learns*
things, and it is intact — what the seat adds is looking at one of them.

## Running it

```sh
cargo build --release

# the lab report's museum, from a fixture — no fleet required
./target/release/orrery --demo

# the gallery screen: read-only, LAN
./target/release/orrery --listen 0.0.0.0:8080

# the operator's posture
./target/release/orrery --operator "$(cat ~/.copal/web.token)"

# the seat, against one node
./target/release/orrery --seat museum-01
./target/release/orrery --seat museum-01 --sixel --fps 10

cargo test
```

## Layout

| | |
|---|---|
| `src/main.rs` | argument handling, the routes, the two postures |
| `src/http.rs` | the subset of HTTP/1.1 this needs, with its caps and timeout |
| `src/json.rs` | just enough JSON to prove a node id is a node id |
| `src/fleet.rs` | the read model, the timed subprocess, the museum fixture |
| `src/verbs.rs` | the allow-list, which is the security boundary |
| `src/rfb.rs` | RFB 3.8 — the one place that opens a socket to a node |
| `src/paint.rs` | a node's screen into a terminal: sixel, and half-blocks |
| `src/seat.rs` | the seat — the tty, the keys, the four faces |
| `assets/wall.html` | the page — one file, no CDN, no build step |

## What it does not do, deliberately

- **It does not proxy VNC.** §III-B of the lab report priced eight VNC sessions
  on 512 MB boards that are also computing and refused them. Control and
  Observe hand the browser the node's own `wayvnc` or `x11vnc`; a console that
  tunnelled pixel traffic would put itself in the exhibit's frame budget.
- **It does not cache state.** `copal fleet state` is the answer, and a cache is
  a second place the fleet's state lives and a thing that goes stale.
- **It does not offer Control on a selection.** §IV-C is emphatic that this is a
  decision rather than a gap: broadcasting keystrokes to eight machines produces
  divergent state nobody can see, and the operation actually wanted is Scene or
  Run — declarative, and each reports per node.

## Still open

- The write verbs build and validate their argv but have never run against a
  live `copal fleet`. The demo fixture reports what it *would* have run.
- **The seat has never met `x11vnc`.** It is proven against a hand-written RFB
  3.8 server (`src/rfb.rs`'s `fake` module, and the wire tests beside it):
  handshake, Raw decode, input encoding and the modifier release are all
  asserted on a real socket. What that does not prove is x11vnc's own choices
  — which security types it offers, and whether it honours a Raw-only encoding
  list rather than sending Tight anyway.
- **Nothing on a node can start `x11vnc`.** The forced command in
  `copal-prep.sh` is a closed `case` over thirteen verbs with, in its own
  words, "no hatch, no raw, and no pass-through" — and no VNC verb among them.
  Until one is added there, Control works only against a node where someone
  started `x11vnc -nopw` by hand.
- **Terminal is a fleet decision, not a console one.** A shell on a node needs
  a second credential with a wider door than the operator certificate has. The
  seat says so rather than offering a tab that cannot work.
- Thumbnails are W5 and the live session is L6. The page renders `n.thumb` the
  moment the read model carries one; nothing else changes.
- A console served *by* the warden disappears when the warden does, which a TUI
  on a laptop does not. The election exists now, so the fix is a second instance
  or a name that follows the role. Unresolved.
- Nothing here has run on a Raspberry Pi. As with all of M4.

MIT. Copyright (c) 2026 Paul Richeson.
