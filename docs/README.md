# The control console — the design set

Five documents in this checkout and one in `copal-alpine-linux`, describing one
thing: **orrery becoming the console the operator stands in front of** — a room
of machines you select from, with verbs that act on the selection, on a Mac and
on a node.

Read in this order.

| | |
|---|---|
| **[console.md](console.md)** | The museum interface. The object model, the screen, the verbs, and what it refuses to offer. Start here. |
| **[surface.md](surface.md)** | One canvas, two platforms. The `Surface` seam, Wayland input, the AppKit backend, the baked keymap, the widgets. |
| **[wire.md](wire.md)** | The credential rule, rewritten. `crypto.rs`, SSH, SFTP, TLS 1.3, RDP — and the one place "no dependencies" genuinely breaks. |
| **[lockdown.md](lockdown.md)** | One list, in one place, with a number on it. The crypto profile, why a standard RDP client cannot reach a node, and what "synchronise the credentials" is allowed to mean. |
| **[media.md](media.md)** | Cards, images and machines. The front end to `copal-prep.sh`, the card ledger, and the manifest. |
| **[plan.md](plan.md)** | The schedule. Eleven phases, what proves each one, the risks ranked, and what would make the plan wrong. |
| **[fleet-control.md](../../copal-alpine-linux/docs/fleet-control.md)** | The node's half. What `copal-prep.sh` grows, and the one thing that turns out to need no change at all. |

---

## The three things worth knowing before reading any of them

**Terminal needs nothing on the node.** The README has said since the seat was
built that a shell needs a second credential the fleet does not have. It has
one: `copal-prep.sh:28383` writes `fleet-human` into
`/etc/ssh/principals/$PI_USER`, and `copal fleet login` has always signed that
principal. One certificate, two principals, two doors.

**The session word resolves the GUI-versus-Control conflict.** `x11` nodes get
Observe and Control over RFB, which `rfb.rs` already speaks. `wayland` nodes get
them over RDP. Neither is a fallback for the other, and on a Wayland node the
console and Control now want the *same* session type rather than opposite ones.

**H.264 is where the no-dependency rule actually breaks**, and it is handled by
not negotiating it — plain bitmap updates with RLE, which is the same choice
`rfb.rs` made when it asked for Raw and CopyRect. Whether the node's RDP server
will agree is the single largest open risk in the plan, and phase 8 is built to
find out before 2,200 lines are written rather than after.
