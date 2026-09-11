# The implementation plan

*Eleven phases, what each one touches, what proves it, and the order — which is
chosen so that the riskiest thing is found out early and the useful thing is
usable long before the end.*

Read [`console.md`](console.md), [`surface.md`](surface.md),
[`wire.md`](wire.md) and [`media.md`](media.md) first; this is the schedule, not
the design. The node's half is
[`fleet-control.md`](../../copal-alpine-linux/docs/fleet-control.md).

---

## The size of it, stated up front

| | today | added | after |
|---|---|---|---|
| orrery, lines of Rust | 7,224 | **~11,400** | ~18,600 |
| of which, protocol and crypto | 903 (`rfb.rs`) | ~7,300 | ~8,200 |
| `copal-prep.sh` | 1.6 MB | ~450 lines | — |
| `tools/`, `Makefile`, `bin/` | — | ~350 lines | — |
| binary, release, stripped | 471 KB | ~900 KB est. | ~1.4 MB |
| resident, serving | 1.1 MB | + one framebuffer per open session | — |

**This is between two and three times the program that exists**, and most of the
new weight is §4–§8 — the wire. That is worth seeing before agreeing to it. The
interface itself, phases 1–3, is about 2,600 lines and is the part that makes
the console usable; everything after it makes a button work that is otherwise
absent.

The phases are ordered so that **stopping after phase 3 leaves a working
console**, stopping after 6 leaves one with a shell and file transfer, and only
phases 7–8 depend on anything unverified.

---

## Phase 0 · The seams

No behaviour changes. This is the refactor that makes the rest additive rather
than invasive.

| | |
|---|---|
| touches | `src/wl.rs`, `src/main.rs`, new `src/surface.rs` |
| adds | ~150 lines |
| does | extract `trait Surface`; `wl::Window` implements it; `Changes` becomes `Vec<Input>` with `Resized` and `Closed`; `--frame` renders through the same path |
| proves | every existing test still passes, `--frame` output is byte-identical to before |

Byte-identical `--frame` output is the whole acceptance test for this phase, and
it is a good one: the specimen exercises every primitive in `draw.rs`, so an
unchanged PPM means the canvas survived the seam.

---

## Phase 1 · Input, and the toolkit

| | |
|---|---|
| touches | `src/wl.rs` |
| adds | `src/ui.rs` ~700, `src/keymap.rs` ~400 (generated), `tools/mkkeymap.py` ~200, `wl.rs` +300 |
| does | `wl_seat`/`wl_pointer`/`wl_keyboard`; `Args::fixed()`; key repeat; the immediate-mode widgets of [surface.md](surface.md) §6 |
| proves | `tools/wl-check.sh` gains input tests driving a second client under weston headless; `ui.rs` has unit tests over synthetic `Input` streams with no compositor at all |

`ui.rs` being testable without a window is the point of immediate mode: a frame
is a pure function of (canvas, theme, inputs, state), so a button press is a
three-line test.

**Watch for:** `wl_fixed` read as an integer (see surface.md §3). It looks like
a broken compositor.

---

## Phase 2 · The Mac backend

| | |
|---|---|
| touches | `src/main.rs`. **`Cargo.toml` does not change**: the frameworks are named with `#[link(name = "AppKit", kind = "framework")]` attributes in `mac.rs` itself, so the dependency list stays empty |
| adds | `src/mac.rs` ~800 |
| does | `Surface` over AppKit through the Objective-C runtime, no runtime class creation; `--scale`; Retina backing factor |
| proves | `--gui` opens on the Mac and draws the specimen; `--frame` still works on both; a screenshot of the Mac window beside the PPM |

This is the phase that lets the rest of the work be *looked at* on the machine
it is being written on, which is why it comes before the interface rather than
after. Writing phases 3–9 against a Wayland-only target means a container round
trip for every visual change.

**Watch for:** the three from [surface.md](surface.md) §4 — `objc_msgSend`
signatures, the CGImage byte-order flags, and the main thread.

---

## Phase 3 · The museum interface

**The first phase that is worth having on its own.**

| | |
|---|---|
| touches | `src/main.rs` (`--gui` stops being a specimen), `src/fleet.rs` (a refresh thread) |
| adds | `src/lab.rs` ~1,100 |
| does | header, rail, tiles, selection, verb bar, results pane — with only the verbs that already exist: Run, Scene, Power, Snapshot, Notify |
| proves | `--frame-state FILE --frame-select a,b` renders every screen in [console.md](console.md) §2 to a PPM, committed, diffed on change |

At the end of this phase there is a native fleet console on a Mac and on a node,
driving the CLI, with a selection model and per-node results. Observe, Control,
Terminal, Exchange, Send and Message are absent — not greyed — because their
transports do not exist yet.

**Gate:** the demo fixture must drive the whole interface. `--demo --gui` with
no fleet at all is how this is developed.

---

## Phase 4 · `crypto.rs` — **built**

| | |
|---|---|
| adds | `src/crypto.rs` 2,971 lines, 35 tests |
| does | SHA-256, SHA-512, HMAC, HKDF (+ TLS 1.3's Expand-Label), X25519, Ed25519 (sign + verify), ChaCha20-Poly1305 in both the IETF and the OpenSSH nonce layouts, AES-256-GCM bitsliced, base64, a DER writer and reader |
| proves | RFC 7748 §5.2 and §6.1, RFC 8032 §7.1, RFC 8439 §2.4.2/§2.5.2/§2.8.2, FIPS 180-4, FIPS 197 C.3, NIST GCM cases 13/14/16, RFC 4231, RFC 5869 |

**The lowest-risk phase in the plan, and the largest.** Every function had a
published answer before it was written. That held: the three bugs the vectors
caught were a hash whose buffer count reset when a call ended mid-block, an
HKDF counter that overflowed on its last legal block, and two test vectors
transcribed from the wrong section of their own RFC.

Two things that are easy to get wrong, and are each now a named test: the
all-zero / small-order X25519 output (`a_small_order_point_is_refused_rather_than_agreed_with`,
five points including both spellings of the identity), and Ed25519's
canonical-`S` check (`a_second_spelling_of_a_signature_is_refused`, which
builds S+L and watches it be turned away).

**What was planned and deliberately not built: P-256 verify and RSA-PSS
verify.** `profile.rs` arrived after this plan did, and it names neither —
`nothing_weak_is_on_any_list` fails the build on `ecdsa` and `ssh-rsa`. Code
for an algorithm the profile forbids is code that can only ever be reached by
a bug, so the two rows came out.

**Discipline:** no secret-dependent branch, no secret-dependent index. The AES
S-box is a circuit rather than a table for exactly that reason, and the bill
for it is measured in the file — 0.5 MB/s against ChaCha's 186. See the note
in §6 of `wire.md`: that number is a constraint on phase 7, not a footnote.

---

## Phase 5 · `ssh.rs` and Terminal

| | |
|---|---|
| adds | `src/ssh.rs` ~1,400, `ssh::fake` ~400, `lab.rs` +200 |
| does | transport, kex, CA host-certificate verification, publickey auth with the operator cert, session channel with pty + shell, rekey |
| proves | `ssh::fake` on a real socket, as `rfb::fake` already does; **and `sshd` in an Alpine container**, which is the row that matters |

Terminal appears in the verb bar. A `term` widget over a real shell.

**Gate, and it is a hard one:** the seat's "Still open" entry is the lesson —
*"what that does not prove is x11vnc's own choices."* A fake server proves
`ssh.rs` agrees with `ssh.rs`. The container test is the phase's real acceptance
criterion, and it is written before the fake, not after.

---

## Phase 6 · `sftp.rs`, Exchange and Send

| | |
|---|---|
| adds | `src/sftp.rs` ~600, `lab.rs` +300 |
| does | SFTP v3, pipelined; the two-pane browser; Send fanned out with per-node results |
| proves | against OpenSSH's own `sftp-server` over a pipe — no fake at all |

**Stopping here is a defensible place to stop.** A console with the wall, a
selection, the CLI's verbs, a shell and file transfer is most of the lab
report's table.

---

## Phase 7 · `tls.rs`

| | |
|---|---|
| adds | `src/tls.rs` ~1,100 |
| does | TLS 1.3 client, X25519, two suites, certificate parsing and pinning |
| proves | against `openssl s_server -tls1_3` — an implementation nobody here wrote |

Independent of phase 8's risk, and cheap to verify, so it goes first and
separately.

---

## Phase 8 · `rdp.rs`, Control and Observe

**Every risk in this plan is in this phase.**

It does **not** begin with code. It begins with:

1. **Find hypr-rdp.** It is not in this repository, not in Alpine's index as far
   as these checkouts know, and has never been run here — H2 of
   [fleet-control.md](../../copal-alpine-linux/docs/fleet-control.md) §5. If it
   cannot be obtained and built, stop and re-plan.
2. **Run it on a node** with Hyprland, and capture a real connection with a
   known-good client (FreeRDP with `/log-level:TRACE`).
3. **Read what it negotiates.** Specifically: does it accept `PROTOCOL_SSL`
   without `PROTOCOL_HYBRID`, and will it send bitmap updates to a client whose
   `Confirm Active` advertises no EGFX and no RemoteFX?

Only if (3) answers yes does the rest of the phase happen:

| | |
|---|---|
| adds | `src/rdp.rs` ~2,200, `lab.rs` +250 |
| does | X.224, MCS/GCC, client info, licensing, capability exchange, finalisation, fastpath in and out, interleaved RLE decode, `cliprdr` text |
| proves | against hypr-rdp on a node, and nothing else — see [wire.md](wire.md) §8 |

**If (3) answers no**, the fallback is stated in advance and is not a defeat:
install `wayvnc` beside Hyprland and point Control at `rfb.rs`, which is written
and tested. The cost is the clipboard and some bandwidth. **Phase 8 is the only
phase whose failure is survivable by design**, and that is deliberate.

---

## Phase 9 · The media pane

| | |
|---|---|
| adds | `src/media.rs` ~600 |
| does | the card ledger, the sequence checks, the three targets, the manifest — [media.md](media.md) |
| needs | the `copal` side of phase 10, so they land together |
| proves | against a scratch fleet: prepare, write to an image rather than a card, and read back a manifest with three rows |

---

## Phase 10 · The `copal-alpine-linux` side

| | |
|---|---|
| touches | `copal-prep.sh` (~450 lines), `tools/copal-fleet-view`, `tools/copal-answers.sh`, `Makefile`, `bin/` |
| does | `facts()` readings, the `remote` and `message` verbs, `copal-remote`, `copal-notify`, `fleet_remote_rdp()`, two answers keys, the manifest tool, the Makefile targets |
| proves | `make lint` — which already extracts `copal-init.sh` from the heredoc and checks it as the file it becomes, and already checks that every `bin/` shortcut names a target that exists |

Some of this is needed earlier: the `session` and `remote` readings (§3 of
fleet-control.md) are wanted by phase 8, and the fixture in `fleet.rs` carries
them from phase 3 so the interface can be drawn against them before a node
emits them.

---

## Phase 11 · The truth pass

Documentation last, and specifically **the corrections**, because three things
this repository currently says will have stopped being true:

| says | becomes |
|---|---|
| *"Terminal is a fleet decision, not a console one."* | it was a console one; the node built the door in stage 16 — [fleet-control.md](../../copal-alpine-linux/docs/fleet-control.md) §1 |
| *"the GUI and the seat's Control want opposite session types. Unresolved."* | resolved: the session word selects the server — §2, ibid. |
| *"Nothing on a node can start `x11vnc`."* | the `remote` verb, bounded four ways — §4, ibid. |
| *"It does not proxy VNC … a console that tunnelled pixel traffic would put itself in the exhibit's frame budget."* | still true for the **web** console; the native one is the operator's own machine and carries one session, which is §III-B's asymmetry unchanged |
| §12 of `fleet-plan.md`: three faces, the third *"served by the warden, read-mostly"* | four, and the new one is neither served nor read-mostly |

That last row is a change to the plan's own architecture table and should be
made deliberately rather than absorbed. **A native console on the operator's Mac
is not served by the warden, which means it does not disappear when the warden
does** — which is, as it happens, the answer to another of the README's open
items.

---

## Risks, ranked

| | risk | if it lands |
|---|---|---|
| **R1** | hypr-rdp will not send bitmap updates to a client with no H.264 | Control over RDP does not happen; `wayvnc` + `rfb.rs` instead. Phase 8 is designed to fail here cheaply, and finds out before writing 2,200 lines. |
| **R2** | hypr-rdp cannot be obtained or built at all | same fallback, found in the first hour of phase 8 |
| **R3** | hand-written crypto has a subtle flaw | mitigated by test vectors and by narrowness — one algorithm per slot, verify-only where possible. Not eliminated. Anything holding this credential on a LAN is a different risk posture from a public service, and that is the argument, not "it is correct". |
| **R4** | `mac.rs` `objc_msgSend` ABI mistakes | mitigated by there being ~25 distinct call signatures and no runtime class creation. Crashes loudly rather than silently. |
| **R5** | the baked US keymap is wrong for the operator | known, stated, `--keymap` takes one value. A real limitation, not a bug. |
| **R6** | 11,400 lines is a lot of program for one person to hold | the phase boundaries are the mitigation: 0–3 is a console, 4–6 is a console with a shell, 7–8 is the part that can be cut. |
| **R7** | the binary triples and a Zero 2 has 512 MB | 1.4 MB resident against 22 MB for the Python prototype is still the argument. Measure on `saskatchewan` at phases 3, 6 and 9 rather than at the end. |

---

## The order, and why it is this one

- **The riskiest thing is phase 8, and it is late** — not because risk should be
  deferred, but because its *investigation* is cheap and its *implementation* is
  expensive. The investigation can be done during any earlier phase; the plan
  says so explicitly. If it fails, nine phases of work are unaffected.
- **The most useful thing is phase 3, and it is early.** A native fleet console
  driving the existing CLI verbs is most of the value, and it is 2,600 lines.
- **The Mac backend is before the interface**, so the interface can be seen
  while it is written.
- **`crypto.rs` is before either protocol**, because writing it twice is the
  actual failure mode of not planning this.
- **Documentation is last and is a correction pass**, because three of this
  repository's honest "still open" entries stop being true and a README that
  keeps them is a README nobody trusts.

---

## What would make this plan wrong

Worth writing down, so it can be checked rather than defended:

- If hypr-rdp turns out to speak something close enough to RFB, or to offer a
  raw framebuffer mode, phases 7 and 8 collapse into a day and `tls.rs` is not
  needed. **Find out first.**
- If a shell on a node turns out to be unwanted — if the operator's actual
  workflow is `copal fleet run` and never a prompt — phases 5 and 6 are 2,400
  lines serving a button nobody presses. The way to test that is phase 3:
  use the console for a week with only the CLI verbs, and see what is missed.
- If the Mac is not where the operator stands, phase 2 is 800 lines of AppKit
  for nothing. It is here because `answers.txt`, the CA, the token ledger and
  the card reader are all there — but that is an inference from the repository,
  not something anyone said.
