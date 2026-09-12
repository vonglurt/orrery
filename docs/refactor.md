# What to refactor, and what to leave alone

*Written at the end of the eleven phases, from inside the code rather than from
the plan. Each entry says what is wrong, what it costs today, and what it would
cost to fix — because a refactoring list with no prices on it is a wish list.*

Ranked by **value divided by risk**, which is not the same as ranked by how
annoying they are.

---

## The five worth doing

### 1 · One SSH connection per node, not one per verb

**What.** `ssh::Session` owns a `Conn` and a single channel. `sftp::Sftp::open`
therefore opens a *second* TCP connection to the same node and repeats the
whole ceremony: version exchange, key exchange, certificate check,
authentication. Terminal and Exchange against one node are two handshakes; Send
to eight nodes is eight.

**What it costs.** A curve25519 exchange plus an Ed25519 verification plus an
Ed25519 signature is about 1.5 ms of arithmetic on this Mac and perhaps 25 ms
on a Zero 2 — but the round trips dominate: measured end to end, opening a
session is close to a second. Send to eight nodes spends eight of them.

**What it would cost to fix.** `Conn` already multiplexes in the protocol's own
terms; what is single is the `Session` wrapper. Splitting it into `Conn` (owns
the socket, dispatches by channel id) and `Channel` (owns a window and a
buffer) is perhaps 150 lines moved and 60 written, and `sftp.rs` and the
Terminal pane both become users of a pool. **The tests already exist**: the fake
server and the real `sshd` both speak the multi-channel protocol today.

**Why it is first.** It is the only item on this list a person would notice.

### 2 · Blit rows, not pixels

**What.** `screen.rs` draws a remote desktop with `ui.c.set(x, y, p)` per
pixel. At 1280×720 scaled into a window that is about a million calls a frame,
each one a bounds check and a multiply.

**What it costs.** It is the reason Control feels heavy at full window size.
The protocol is not the bottleneck; the blit is.

**What it would cost to fix.** `Canvas` needs one method — a row slice — and
the loop needs to walk source rows once. Thirty lines, no design questions, and
the existing frame tests cover it.

### 3 · One writer, not three

**What.** `ssh::Buf`, `tls::W` and `rdp::W` are the same object three times:
push a byte, push a big-endian or little-endian integer, push a
length-prefixed string. They differ in which widths they name and which
endianness is the default.

**What it costs.** About 150 lines of duplication, and — more to the point —
three places to get a length wrong, which is the single most common protocol
bug in all three files.

**What it would cost to fix.** One `Bytes` type in a new small module with
`u8/be16/le16/be32/le32/raw/zeros`, and each protocol keeping only its own
length-prefix helpers (`ssh::string`, `tls::v24`, `rdp` has none). Half a day,
and every existing test exercises it immediately.

**The counter-argument, which is real.** Each protocol's writer reads like that
protocol. `w.str(ED25519).string(&sig)` says SSH; `w.v24(&list)` says TLS. A
shared writer would be slightly less legible at every call site in exchange for
being right in one place. That trade is usually worth taking and it is worth
saying out loud rather than pretending there is no cost.

### 4 · One pane lifecycle

**What.** The window holds three panes — `media::Job`, `files::Files`,
`screen::Screen` — and each has its own `Option<T>`, its own "opening on a
thread" channel, its own close flag in `gui_loop`, and its own `Pane` enum with
slightly different variants. `main.rs`'s frame loop has grown three nearly
identical blocks.

**What it costs.** About 120 lines of `main.rs` that a reader has to check are
the same rather than being told they are. It is also where the next pane will
be added wrong.

**What it would cost to fix.** A `trait Pane { fn pump(&mut self); fn draw(&mut
self, ui, at) -> Verdict; fn close(&mut self); }` and one `Option<Box<dyn
Pane>>`, plus one `open_on_thread` helper replacing `open_shell`,
`open_desktop` and the browser's worker spawn. Perhaps 200 lines moved.

**Why it is not first.** It makes the program easier to extend and does not
make it better at anything it already does.

### 5 · One fixture script for the container checks

**What.** `ssh-check.sh`, `tls-check.sh` and `rdp-check.sh` each generate a CA,
a node certificate and an operator certificate with the same eight `openssl`
commands.

**What it costs.** Three copies that have already drifted once: the SSH one
issues OpenSSH certificates and the other two X.509, which is correct, but the
*names* and the validity windows are copy-pasted and could disagree without
anything noticing.

**What it would cost to fix.** `tools/fleet-fixture.sh` sourced by all three,
forty lines. Low value, near-zero risk, and it is the kind of thing that stops
being done once there are four scripts.

---

## Three that look like refactors and are not

### `lab.rs` is 1,300 lines

It holds the read model's parse, the layout, the drawing and the input. That
sounds like three files. It is not, because **the layout is the argument**: the
reason a tile is 150 pixels wide is the reason the verb bar can hold eight
buttons, and splitting them would put the two halves of one decision in two
files. The parse could leave — it is genuinely separate — and would take about
90 lines with it. The rest should stay.

### `crypto.rs` is 3,000 lines

It is one module because it is one subject and because `nothing_here_is_off_the_profile`
has to see all of it. Splitting it into `hash.rs`, `curve.rs`, `aead.rs` would
be tidier to look at and would make the profile test reach across four modules
to say one thing.

### The two certificate readers

`ssh.rs` reads OpenSSH certificates and `x509.rs` reads X.509, and both check a
signature, a validity window and a name. They are not the same code and should
not become the same code: the formats share nothing but the idea, and a common
abstraction over them would be an abstraction over one sentence.

---

## The bug class this list exists because of

`fcntl` is variadic in C — `int fcntl(int, int, ...)` — and `pty.rs` declared it
with a fixed third argument. It compiled, linked and ran; on aarch64 Darwin the
variadic convention puts that argument on the stack while a fixed one goes in a
register, so `F_SETFL` never received `O_NONBLOCK`. **Every read that was meant
to return immediately blocked until the far end spoke.**

It hid for two phases because every read happened *after* a question had been
asked, and the answer was always on its way. It appeared the moment `sftp.rs`
read a pipe *before* asking anything — to drain answers between pipelined
writes — and then it was a hang rather than a wrong answer.

Two tests now assert the property directly, one in each file: ask for nothing,
and require the read to come back anyway. **Look for the same shape elsewhere**:
any `extern "C"` declaration of a function whose C prototype ends in `...`
is the same bug waiting. In this repository that is `fcntl` and nothing else,
and it is now declared `fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int`.

---

## The two performance numbers worth knowing before optimising anything

**AES-256-GCM at 0.5 MB/s** (`crypto.rs`, `how_fast_the_ciphers_are`). The
four-block bitslice would make it roughly three times faster and still eighty
times slower than ChaCha. **The right fix is not to make AES faster** — it is
that nothing in the fleet should ever choose it, which `tls.rs` already
ensures by offering one suite. If a peer ever forces it, revisit; until then
this is a compatibility path and its speed is the wrong thing to work on.

**`Fe::sq` is `mul(self, self)`.** A dedicated squaring is about a third
cheaper, and the inversion chain does 254 of them. That is perhaps 25% off
every X25519 exchange and every Ed25519 operation — which is 0.15 ms on this
Mac and a few milliseconds on a node. It is a real saving and it is also a
second formula to get wrong in the file where being wrong is worst. **Do it
only with a test that compares every result against `mul`**, over a few
thousand random elements; then it costs nothing and proves itself.

---

## What the next person should not do

- **Do not add a crate.** The reason is in `Cargo.toml` and it has not changed:
  `copal-build` compiles this on a node whose LAN never reaches the internet.
- **Do not widen `profile.rs` to make something work.** The list is the
  security boundary; a peer that cannot meet it is a peer this console is
  supposed to refuse. `nothing_weak_is_on_any_list` will fail the build, and
  that is the feature.
- **Do not make the panes into windows.** One pane at a time, never a window
  inside the window, is a decision `console.md` argues for and every pane here
  follows.
- **Do not cache the read model.** It is a second place the fleet's state
  lives.
