# orrery

The Copal fleet's console — the wall, the seat, the museum interface, and the
verbs that act on a room full of machines.

An orrery is a clockwork model that shows many bodies at once and lets you turn
a handle to see where they will be. That is the wall. The rest of this program
grew out of wanting to reach through it.

## What it is

§12 of `copal-alpine-linux/docs/fleet-plan.md` names three faces on one read
model. This repository is two of them and then some:

| | | |
|---|---|---|
| **the wall** | `orrery --listen` | the web console: a page the gallery screen and a phone can open |
| **the seat** | `orrery --seat ID` | one node, full screen, in the terminal you are already in |
| **the museum interface** | `orrery --gui` | a native window: a room of machines, a row of verbs, and the panes they open |

All three render the same read model, and that model is `copal fleet state` and
nothing else.

## The rule it inherits

`copal-fleet-console.py`'s acceptance test was architectural: it opens no
socket, holds no credential, knows no subject names, and every screen is a
rendering of a command a person could have typed.

**Two of those four are no longer true, and the change was deliberate.** The
seat opens a socket to one node's screen; the museum interface opens SSH, SFTP
and RDP with the operator's own certificate. What is intact is the part that
mattered: **how the console learns things.** Every node id, address and fact
comes out of `copal fleet state`; a node that is not in that document cannot be
dialled, and there is no second way of knowing anything. A console that
discovered nodes for itself would be a second fleet.

## Two postures

Read-mostly is a default, not a ceiling. A gallery screen and a phone on a
lanyard are two audiences with different rights.

| | | |
|---|---|---|
| **gallery** | default | State, thumbnails. No verb that changes anything. |
| **operator** | `--operator TOKEN` | The write verbs as well. |

Without a token the write route **does not exist** rather than answering 403 —
a route that says "forbidden" tells a scanner it is there. In the native
window the same rule takes a different shape: the gallery posture's dispatch
table is empty, so a keystroke that would have been a verb finds nothing to
call.

## No dependencies, and that is the design

`copal-build` compiles the checkouts in `~/code` on the node itself, and a
fleet node is on a LAN that invariant 6 says never reaches the internet. A
dependency is therefore a crate fetch that fails on the one machine this is
meant to run on.

So everything is here: HTTP, JSON, Wayland, AppKit through the Objective-C
runtime, a software canvas, two bitmap fonts, SHA-2, X25519, Ed25519,
ChaCha20-Poly1305, AES-GCM, SSH-2, SFTP, TLS 1.3, X.509 and RDP. **25,700
lines of Rust and an empty `[dependencies]`.**

| | |
|---|---|
| Binary, release, stripped | 817 KB |
| Resident, serving the wall | 1.1 MB — against 22 MB for the Python prototype |
| Tests | **321**, and **342** on Linux where the compositor tests run |

The honest exception is declaring functions from libraries the machine already
has: `src/sys.rs` names nine libc functions for Wayland, `src/mac.rs` names
AppKit and CoreGraphics, `src/pty.rs` names `openpty`. Nothing is fetched and
nothing is vendored.

## The museum interface

```sh
./target/release/orrery --gui --demo --operator x
```

A room of machines. Click tiles, or click a scene or a tag in the rail to
select everything running it. The bar under the lab shows the verbs that can
apply to *that* selection, and the footer says how many are not built.

| verb | | what it opens |
|---|---|---|
| **Control** `c` | one node | the node's desktop, over RDP with mutual TLS |
| **Terminal** `t` | one node | a shell, in a pane, with no password asked |
| **Exchange** `e` | one node | a two-pane file browser over SFTP |
| **Send** `s` | many | one file to every selected node, with a per-node result |
| **Message** `m` | many | a banner on every selected screen |
| **Run** `r` · **Scene** `S` | many | the CLI's own verbs, per node |
| **Snapshot** `k` · **Power** `p` | many | as above, and `power on` is the supply rather than ssh |
| **Observe** `o` | — | **refused**, and §5 of `docs/console.md` says why |

A verb with no transport is **absent rather than greyed**: the status line says
how many, and the bar never draws a button that cannot work. There is one pane
at a time and never a window inside the window.

`Cmd-1` and `Cmd-2` switch between the Lab and **Media**, which is the SD-card
ledger: which card carries which token, which have been written, and a pane
that runs `copal-prep.sh` on a real terminal so that its two typed `ERASE`
confirmations happen where they always did. Nothing is answered on the
operator's behalf and there is no `--yes`.

`--frame PATH` renders any of it to a PPM and exits — no compositor, no window,
no node. Every screenshot in `docs/` is a file this produced.

## The wire

Five protocols, written by hand, each proved against somebody else's
implementation rather than against a fake of its own assumptions.

| | | proved against |
|---|---|---|
| `src/crypto.rs` | the primitives | published test vectors — RFC 7748, 8032, 8439, FIPS 180-4 and 197, the NIST GCM cases |
| `src/ssh.rs` | SSH-2, certificates only | **a real `sshd`** in a container, plus a hand-written server for the lies a real one will not tell |
| `src/sftp.rs` | SFTP v3, pipelined | **OpenSSH's own `sftp-server`** over a pipe — no fake at all |
| `src/tls.rs` + `src/x509.rs` | TLS 1.3, one suite | **`openssl s_server`**, including mutual TLS and an AES-only server being refused |
| `src/rdp.rs` | RDP over TLS | **xrdp** — 1024×768, 137 rectangles, all of them RLE compressed |

`src/profile.rs` is the whitelist: one key exchange, one host-key algorithm,
one signature algorithm, one cipher offered. A test fails the build if `sha1`,
`cbc`, `rc4`, `ssh-rsa` or `ecdsa` ever appears on a list, and
`orrery --profile-sshd` prints the same list as `sshd_config` lines —
which `copal-prep.sh` writes into the node and `make lint` checks for drift.
**A whitelist that exists in a Rust file and again in a shell heredoc is two
whitelists.**

The host key must be a certificate signed by the fleet CA, naming that node,
valid now, with the far end proving it holds the key. There is no
trust-on-first-use, no prompt, and no bare-key branch to fall back to.

## The seat

The wall is eight nodes at a glance. The seat is one node, full screen, in the
terminal you are already in.

```sh
./target/release/orrery --seat museum-01
```

| | | |
|---|---|---|
| **Observe** | `o` | the node's screen, read-only |
| **Control** | `c` | keyboard and pointer as well; `Ctrl-]` lets go |
| **Exchange** | `e` | not built here — the native window has it |
| **Terminal** | `t` | not built here — the native window has it |

It renders with half-blocks (`U+2580` and 24-bit colour) anywhere, and with
sixel where the terminal has it. `--fps` asks the node for a frame rate; the
default is 6, because §III-B's finding is that frame rate is the expensive
axis.

The seat speaks RFB security type `none` only — VNC's own authentication is
56-bit DES and an eight-character password, which is not security — and asks
for Raw and CopyRect only, because every compressed encoding wants zlib.

## The native window, underneath

`--gui` is a Wayland client on a node and an AppKit window on a Mac, behind one
`Surface` trait. **The drawing is all software and there is no renderer
underneath it**: a shared-memory surface is a flat array of pixels, so
`draw.rs` writes them. On a Zero 2 that is the right trade twice — a wall of
flat rectangles and small text costs almost nothing to rasterise, and the EGL
stack it avoids is both a dependency and a thing that fails differently on
every board.

**The letters are baked in.** `src/font.rs` carries two faces of the X.Org
*misc-fixed* font, generated from the PCF by `tools/mkfont.py` and committed,
because `copal-build` on a node must not need Python or a font package. Past
Latin-1 exactly seven characters are baked, each one the console actually
prints — and a test fails the build if the interface reaches for an eighth.

**The session word decides the protocol, and that used to be unresolved.**
`/etc/copal/session` holds one word: `x11` means x11vnc and RFB, `wayland`
means an RDP server and `rdp.rs`. Neither is a fallback for the other. The
console reads the node's own `remote` reading rather than probing a port —
`off`, `not installed`, `vnc:5900` and `rdp:3389` are four different sentences.

## Running it

```sh
cargo build --release

# the lab report's museum, from a fixture -- no fleet required
./target/release/orrery --demo                       # the wall, on a port
./target/release/orrery --gui --demo --operator x    # the museum interface

# against a fleet
./target/release/orrery --listen 0.0.0.0:8080
./target/release/orrery --gui --operator x --copal ~/code/copal-alpine-linux
./target/release/orrery --seat museum-01

# the node's sshd policy, rendered from src/profile.rs
./target/release/orrery --profile-sshd

cargo test
```

The suites that need somebody else's server run in containers, one script
each, and they step aside when what they need is absent:

```sh
docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
    -e CARGO_TARGET_DIR=/tmp/t rust:alpine sh /w/tools/wl-check.sh   # weston
#                                             ... /w/tools/ssh-check.sh  # sshd
#                                             ... /w/tools/tls-check.sh  # openssl
#                                             ... /w/tools/rdp-check.sh  # xrdp
```

## Layout

| | |
|---|---|
| `src/main.rs` | argument handling, the routes, the two postures, the frame loop |
| `src/http.rs` · `src/json.rs` | the subsets of HTTP/1.1 and JSON this needs |
| `src/fleet.rs` | the read model, the timed subprocess, the museum fixture |
| `src/verbs.rs` | the allow-list, which is the security boundary |
| `src/profile.rs` | the crypto whitelist, versioned, shared with the node |
| `src/crypto.rs` | the primitives, every one with a published test vector |
| `src/ssh.rs` · `src/sftp.rs` | the transport, and files over it |
| `src/tls.rs` · `src/x509.rs` | TLS 1.3, and the certificates it checks |
| `src/rdp.rs` · `src/rfb.rs` | the two remote-desktop protocols |
| `src/surface.rs` | the seam: a window, its pixels, and its input |
| `src/wl.rs` · `src/mac.rs` | Wayland by hand, and AppKit through the runtime |
| `src/draw.rs` · `src/font.rs` · `src/ui.rs` | the canvas, the glyphs, the toolkit |
| `src/lab.rs` · `src/nav.rs` | the museum interface and the view strip |
| `src/media.rs` · `src/pty.rs` | the card ledger, and a child on a real terminal |
| `src/files.rs` · `src/screen.rs` | the file browser, and a desktop in a pane |
| `src/seat.rs` · `src/paint.rs` | the terminal seat: the tty, the keys, the pixels |
| `src/keymap.rs` · `src/sys.rs` | the keyboard, and the syscalls `std` does not expose |
| `tools/mkfont.py` | turns an X11 PCF font into `src/font.rs` |
| `tools/*-check.sh` | the suites against a compositor, an sshd, openssl and xrdp |
| `assets/wall.html` | the page — one file, no CDN, no build step |
| `docs/` | the design set; `docs/plan.md` is the board, `docs/writeup.html` is the account of the build, `docs/refactor.md` is what to do next |

## What it does not do, deliberately

- **The wall does not proxy VNC.** §III-B priced eight VNC sessions on 512 MB
  boards that are also computing and refused them. The page hands the browser
  the node's own server; a console that tunnelled pixel traffic would put
  itself in the exhibit's frame budget.
- **It does not cache state.** `copal fleet state` is the answer, and a cache
  is a second place the fleet's state lives and a thing that goes stale.
- **It does not offer Control on a selection.** Broadcasting keystrokes to
  eight machines produces divergent state nobody can see; the operation
  actually wanted is Scene or Run, which are declarative and report per node.
- **It never mints a credential.** The fleet CA signs; this console
  distributes and verifies. A program holding both would be a program worth
  stealing.
- **It does not decode H.264.** A video codec has no small correct subset and
  `libavcodec` is not on an Alpine node, so RDP negotiates plain bitmaps with
  RLE. `docs/wire.md` §6 has the arithmetic.

## Still open

- **Nothing here has run on a Raspberry Pi.** The whole program cross-compiles
  to `aarch64-unknown-linux-musl` and its Linux suite passes in a container on
  this Mac, which is not the same claim.
- **No node has ever run `hypr-rdp`.** `rdp.rs` is proved against xrdp, which
  is a different implementation by other people and is not the one a Wayland
  node will run. R1 and R2 in `docs/wire.md` are unchanged.
- **An RDP server may refuse an Ed25519 certificate.** FreeRDP's does: it
  derives channel bindings by hashing the certificate with the digest named in
  its signature algorithm, and Ed25519 names none. R5 in `docs/wire.md`.
- **The operator has no X.509 certificate yet.** Mutual TLS works and the fleet
  CA is an SSH CA; issuing X.509 from it needs an ASN.1 writer beside
  `sign_one()`. Until then Control refuses with a sentence naming the file it
  wanted.
- **AES-256-GCM runs at half a megabyte a second.** It is a constant-time
  circuit rather than a table, and it is the compatibility path rather than a
  transport. R4 in `docs/wire.md`, and `tls.rs` offers ChaCha alone because of
  it.
- **Observe is refused rather than unbuilt**, and the difference matters:
  nothing produces a thumbnail and nothing will, because a node capturing its
  screen every few seconds is a capture daemon on every machine in the museum.
  `fleet-control.md` §9 records the shape that would fit instead.
- **A console served *by* the warden disappears when the warden does.** The
  election exists, so the fix is a second instance or a name that follows the
  role. Unresolved — and the native window sidesteps it by running on the
  operator's own machine.

MIT. Copyright (c) 2026 Paul Richeson.
