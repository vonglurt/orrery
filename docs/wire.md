# The credential rule, rewritten

*What orrery is now allowed to hold, what it speaks, and the one place where
"no dependencies" genuinely breaks.*

---

## 1 · The rule as it stood, and why it is changing

`copal-fleet-console.py`'s acceptance test was architectural and this program
inherited it verbatim: **it opens no socket, holds no credential, knows no
subject names, and every screen is a rendering of a command a person could have
typed.**

`rfb.rs` was the first exception and it was argued narrowly. The seat opens a
socket, but to an address that came out of `copal fleet state` and nowhere else;
it holds no credential, because it speaks RFB security type `none` only; and
what it carries is pixels, not knowledge. Three clauses of four survived.

Terminal, Exchange, Send and Control do not survive that argument. A shell needs
a key. SFTP needs a key. RDP needs a TLS session and an account. So the rule is
restated rather than quietly dropped, and the restatement is the thing to hold
this program to:

> **orrery uses the operator credential it was pointed at. It never creates one,
> never stores one, never prompts for one, never forwards one, and never
> outlives one.**

Six clauses, each of which is a check somewhere in the code:

| clause | what enforces it |
|---|---|
| never creates | there is no keygen. A missing `~/.copal/fleets/<f>/operator` is an absent button and a sentence, not an offer to make one. `copal fleet login` makes keys; this reads them. |
| never stores | the key is read at connect, used, and the buffer zeroed. Nothing is written to disk except the RDP pin file, which holds a public fingerprint. |
| never prompts | no password field exists anywhere in the program. The node's sshd has `PasswordAuthentication no` under the fleet block; a client that could offer a password could only offer it to something else. |
| never forwards | no agent forwarding, no port forwarding, no X11. The node refuses all three for `copal-fleet`; orrery refuses them for both accounts, so the client is not the weaker end. |
| never outlives | the certificate is valid `--hours` (8 by default). The status line counts down; at zero, sessions close and the session verbs go absent. |
| knows no subject names | unchanged, and now load-bearing: the *bus* subjects are still none of this program's business. SSH and RDP are point-to-point to an address the read model gave. |

**The honest cost.** A process holding an 8-hour operator certificate in memory
is a more interesting target than a process holding nothing, and the fleet's
posture — invariant 6's LAN, a certificate rather than a password — is what
makes that acceptable rather than fine. It is the same trade `copal fleet`
itself makes, in the same file, on the same machine.

---

## 2 · The shape of the job

Four new modules, and the reason they are one job rather than four is the
bottom one:

```
  src/crypto.rs   the primitives           ~2000 lines, every line has a test vector
       │
       ├── src/ssh.rs    SSH-2 transport + auth + channels   ~1400
       │        └── src/sftp.rs   SFTP v3                    ~600
       │
       └── src/tls.rs    TLS 1.3 client, one suite           ~1100
                └── src/rdp.rs    RDP client                 ~2200
```

**SSH and TLS want the same arithmetic.** This is the structural fact that makes
the whole thing tractable:

| primitive | SSH-2 | TLS 1.3 | RDP above TLS |
|---|---|---|---|
| SHA-256 | exchange hash, signatures | transcript, HKDF | — |
| SHA-512 | Ed25519 internals | Ed25519 internals | — |
| HMAC-SHA256 | — | HKDF-Extract/Expand | — |
| X25519 | `curve25519-sha256` kex | `key_share` | — |
| Ed25519 | host key & certificate | `CertificateVerify` | — |
| ChaCha20-Poly1305 | `chacha20-poly1305@openssh.com` | `TLS_CHACHA20_POLY1305_SHA256` | — |
| AES-128-GCM | `aes128-gcm@openssh.com` | `TLS_AES_128_GCM_SHA256` | — |
| base64 / DER / ASN.1 | OpenSSH key files | X.509 certificates | — |
| CRC-32, RLE | — | — | bitmap updates |

Write `crypto.rs` once and the second protocol is a wire format rather than a
cryptosystem. **And it is the least risky part of this plan, not the most**,
because every single function in it has a published answer: RFC 7748 §5.2 for
X25519, RFC 8032 §7.1 for Ed25519, RFC 8439 §2.8.2 for ChaCha20-Poly1305, the
NIST GCM known-answer tests, FIPS 180-4 for SHA-2. A module where every line is
checked against a vector somebody else published is a module that is either
right or obviously wrong.

**Constant time is a discipline here, not a library feature.** No secret-
dependent branch, no secret-dependent table index, no early return from a
comparison. Field arithmetic over 2²⁵⁵−19 in 5×51-bit or 10×26-bit limbs with
no reduction branches; tag comparison by OR-accumulating a difference. A
textbook AES with T-tables is a cache-timing oracle and is not what goes in.

**Which is why ChaCha20-Poly1305 is the default everywhere.** It is constant
time by construction, it is fast in software on a core with no AES
instructions, and a Zero 2 has no AES instructions. AES-128-GCM is implemented
because the far end may insist — an IronRDP server very likely will, for
TLS — and is bitsliced rather than tabulated, which is slower to write and the
only version worth having.

---

## 3 · `ssh.rs` — one algorithm per slot

`rfb.rs` asks for Raw and CopyRect and nothing else, because a LAN carrying one
session does not need zlib and every encoding is a decoder that can be wrong.
The same principle, harder:

| slot | offered | and nothing else |
|---|---|---|
| key exchange | `curve25519-sha256` | no DH group exchange, no NIST curves, no post-quantum hybrid |
| host key | `ssh-ed25519-cert-v01@openssh.com`, `ssh-ed25519` | no RSA, no ECDSA |
| cipher | `chacha20-poly1305@openssh.com`, `aes128-gcm@openssh.com` | no CBC, no CTR-plus-MAC |
| MAC | none — both ciphers are AEAD | no `hmac-sha2-256-etm` path to get wrong |
| compression | `none` | no zlib |
| auth | `publickey` with a certificate | **no password, no keyboard-interactive** |

### Host verification is the fleet's, and it is better than `ssh`'s

This is the part worth being pleased about. `ssh` on the operator's Mac checks a
node against `known_hosts` — trust on first use, a file that accumulates, and a
warning everybody learns to ignore when a node is reimaged.

orrery has the fleet's certificate authority sitting at
`~/.copal/ca/<fleet>_ca.pub`, and `sign_one()` in `copal-fleet.sh` installed a
*host certificate* on every enrolled node, valid 90 days, with principals
`<id>,<id>.local,<address>`. So the check is:

1. the host key blob is an `ssh-ed25519-cert-v01` certificate;
2. its `signature key` is this fleet's CA;
3. the Ed25519 signature over the certificate body verifies;
4. `valid after ≤ now ≤ valid before`;
5. the node id from the read model is among its principals;
6. `critical options` is empty and every extension is one we understand.

A reimaged node gets a new certificate and connects. A machine that is not this
fleet's fails at step 2, which is §5 of the plan working as designed. **There is
no first-use prompt and no `known_hosts`, because there is nothing to decide.**

### Authentication

`publickey`, with the certificate at `~/.copal/fleets/<f>/operator-cert.pub` and
the private half at `operator` — an unencrypted OpenSSH-format Ed25519 key,
because `cmd_login` creates it with `-N ''`. Parsing it is the
`openssh-key-v1` container: magic, `none` cipher, `none` kdf, one public key,
one private blob with its two check-ints. Roughly 120 lines and it refuses any
`ciphername` other than `none` rather than growing a passphrase prompt.

Two users, one certificate:

| user | principal | lands in | gives |
|---|---|---|---|
| `copal-fleet@node` | `fleet-operator` | `ForceCommand copal-fleet-exec` | the verb list. What `copal fleet run` already uses. |
| `<login user>@node` | `fleet-human` | a real shell | Terminal, and the SFTP subsystem for Exchange and Send |

Both already exist on the node. `copal-prep.sh:28382-28383` writes both
principal files; `cmd_login` signs both principals. **Nothing in `copal-prep.sh`
has to change for SSH to work**, which is the most useful sentence in this
document.

### Channels, and the two that are refused

`session` with `pty-req` + `shell` is Terminal. `session` with
`subsystem sftp` is Exchange and Send. `exec` is there because it is how a
verb would be run without going through the shell, and is used for nothing yet.

`direct-tcpip` and `tcpip-forward` are **not implemented**. A console that can
open a tunnel is a console that can reach anything the node can reach, and the
node's own sshd already says `AllowTcpForwarding no` for the service account.
The client agreeing is not redundant: it means a bug here cannot become one.

### Rekeying is implemented, and is the thing most likely to be forgotten

A session that transfers a gigabyte or runs for an hour must rekey, and an
implementation that does not simply stops working — usually during the one long
file transfer that mattered. `KEXINIT` is re-sent at 1 GiB or 60 minutes,
whichever is first, and the test for it drives a `fake` server that demands one
at 64 KiB so the path is exercised in a second rather than an hour.

---

## 4 · `sftp.rs` — Exchange and Send

SFTP protocol version 3, which is what OpenSSH's server speaks. Eleven packet
types out of a possible forty:

`INIT`/`VERSION`, `OPEN`, `CLOSE`, `READ`, `WRITE`, `OPENDIR`, `READDIR`,
`STAT`/`LSTAT`, `REALPATH`, `RENAME`, `REMOVE`, `MKDIR` — plus `STATUS`,
`HANDLE`, `DATA` and `NAME` coming back.

No symlink creation, no `SETSTAT`, no `FSETSTAT`, no extensions. Reads and
writes are pipelined — 32 KiB requests, 16 outstanding — because a serial
request-per-block over a LAN with a 1 ms round trip caps at about 30 MB/s and
the boards can do better than that.

**Send on a selection is one transfer per node, sequential**, for the same
reason `verb_each` is sequential: eight SSH sessions opened at once from a Pi is
a worse morning than eight opened in a row, and the operator gets a per-node
result either way.

---

## 5 · `tls.rs` — TLS 1.3, client, one suite

Required because RDP's only acceptable security layer is TLS, and because
IronRDP will not speak the alternative.

**TLS 1.3 only.** `supported_versions` offers `0x0304` and nothing else, so
there is no downgrade dance, no `ChangeCipherSpec` state machine that matters,
no RSA key transport, no renegotiation, and no TLS 1.2 record-layer MAC-then-
encrypt to get wrong. A server that cannot do 1.3 is refused with a sentence.

| | |
|---|---|
| key share | X25519 only |
| suites | `TLS_CHACHA20_POLY1305_SHA256`, `TLS_AES_128_GCM_SHA256` |
| signature algorithms | `ed25519`, `ecdsa_secp256r1_sha256`, `rsa_pss_rsae_sha256` |
| not implemented | resumption, 0-RTT, client certificates, ALPN, OCSP, revocation, session tickets |

`ecdsa_secp256r1` and `rsa_pss` are offered because a self-signed server
certificate is very often one of those two and orrery does not get to choose
what the far end generated. That means P-256 verification and RSA-PSS
verification in `crypto.rs` — both verify-only, which is meaningfully less code
than signing, and both constant-time-irrelevant because there is no secret.

### Certificate handling: pinned, not validated

hypr-rdp will present a self-signed certificate. There is no web PKI here and
there should not be one. So:

- the certificate chain is parsed only far enough to extract the
  `subjectPublicKeyInfo` and check the signature over the handshake;
- its SHA-256 fingerprint is looked up in
  `~/.copal/fleets/<fleet>/rdp-hosts` — one line per node id;
- **unknown** → shown to the operator with the node id and the fingerprint, and
  recorded on acceptance;
- **changed** → refused, loudly, and not acceptable from the dialog. Clearing it
  is editing the file, which is a deliberate act with a timestamp on it.

This is trust on first use, which is weaker than the SSH story above, and it is
weaker for a concrete reason: the fleet CA signs SSH host certificates and does
not currently issue X.509. **That is a solvable problem and it is not solved
here** — see [`fleet-control.md`](../../copal-alpine-linux/docs/fleet-control.md) §6,
which proposes issuing hypr-rdp's certificate from the same CA at enrolment so
that RDP gets the same check SSH has. Until then, pinning.

---

## 6 · `rdp.rs` — and the place this plan is actually at risk

### The sequence

RDP is a stack of four protocols that were designed in different decades, and a
client is mostly the correct order of things:

1. **X.224** connection request carrying `RDP_NEG_REQ`, requesting
   `PROTOCOL_SSL` (`0x00000001`) only.
2. **TLS**, from §5. Everything after this is inside it.
3. **MCS** connect-initial, carrying a GCC conference-create request with four
   client data blocks: **core** (version, desktop width/height, colour depth,
   `earlyCapabilityFlags`), **security**, **network** (the virtual channels
   asked for), **cluster**.
4. **MCS** erect-domain, attach-user, and one channel-join per channel.
5. **Client Info PDU** — the account. Domain, username, and an empty password,
   because the TLS-only path with an account the node already trusts is the
   door; see below.
6. **Licensing** — accept the server's "no licence required" error PDU and move
   on. Every non-Windows server sends this.
7. **Capability exchange** — `Demand Active` in, `Confirm Active` out, carrying
   the capability sets: general, bitmap, order, pointer, input, virtual channel,
   and `bitmapCodecs`.
8. **Connection finalisation** — synchronise, control cooperate, control
   request, font list, font map. Five PDUs in a fixed order that mean nothing
   and are mandatory.
9. **Steady state** — fastpath input PDUs out, fastpath update PDUs in.

Virtual channels: `cliprdr` for the clipboard, text only, both directions —
small, and the difference between a usable session and a demo. `rdpsnd` is
**not** implemented; there is no audio, and a console that carried audio from
eight boards would be back in the frame budget §III-B refused.

`drdynvc` is where EGFX would live, and it is not asked for. Which brings this
to the actual problem.

### H.264 is where "no dependencies" breaks, and it is not close

hypr-rdp's headline feature is VA-API-accelerated H.264 over EGFX. **A H.264
decoder is not a thing to hand-write in this program.** It is a video codec:
CABAC, deblocking, motion compensation, a decoded picture buffer. It is larger
than everything else in this document combined, it has no small correct subset,
and linking `libavcodec` is a dependency on the one machine that cannot fetch
one — and, unlike libc and AppKit, is not already present on an Alpine node
unless something put it there.

So EGFX is not negotiated, and what is asked for instead is the oldest and
simplest thing RDP has: **bitmap updates**, with interleaved RLE compression.
About 200 lines of decoder, well specified in MS-RDPBCGR §3.1.9, and the exact
shape of the choice `rfb.rs` already made when it asked for Raw and CopyRect.

**The arithmetic, stated rather than hoped:**

| | |
|---|---|
| a 1280×720 frame, 24-bit, uncompressed | 2.6 MB |
| at 6 fps, full-screen changes | 126 Mbit/s — a Zero 2's 100 Mbit link cannot |
| interleaved RLE on a typical desktop | 5–15× — call it 10–25 Mbit/s |
| what actually travels: damage rectangles only | a text cursor is a few KB a frame |

Which is to say: **Control over RDP will be good for a desktop and bad for
video.** Dragging a window will be smooth; playing the exhibit's video full
screen will be a slideshow. `--fps 6` remains the default for the reason §III-B
found — frame rate is the expensive axis — and the honest description of this
feature is "you can drive a node from across the room", not "you can watch it".

### The two risks, named

**R1 — hypr-rdp may not offer the bitmap path.** A server written around
H.264/EGFX may negotiate EGFX or nothing. If it does, the options are: a patch
upstream to fall back to bitmap updates, configuring it to prefer the legacy
path, or running `wayvnc` beside it and using `rfb.rs`, which already works.
**This must be tested against a real hypr-rdp before `rdp.rs` is written**, and
the plan puts that test first for exactly this reason.

**R2 — hypr-rdp has not been verified to exist.** The description this design
was given — IronRDP, VA-API H.264, PipeWire audio, wlr-screencopy-v1, Hyprland
0.54+ — is detailed and coherent, and there is no reference to it anywhere in
`copal-alpine-linux`, it is not in Alpine's package index as far as this
checkout knows, and nothing here has talked to one. Every claim in this
document about what it will accept is therefore an assumption. Phase 8 begins
by installing it on a node and reading what it actually negotiates.

### NLA is refused

Network Level Authentication means CredSSP, which means SPNEGO, which means
NTLM or Kerberos. NTLM means MD4, RC4, and a credential-forwarding design whose
entire purpose is to carry a password to a machine before that machine has
proved anything. It is a large amount of code to implement a worse security
property than the one already in hand.

`PROTOCOL_SSL` without `PROTOCOL_HYBRID`, an account the node already has, and
invariant 6's LAN. If hypr-rdp requires NLA, RDP does not happen and `wayvnc`
does — which is a smaller loss than it sounds, because `rfb.rs` is written.

---

## 7 · What is deliberately never built

- **A password field.** Anywhere. See §1.
- **An SSH server.** Nothing connects *to* orrery.
- **Port forwarding, agent forwarding, X11 forwarding.**
- **Audio.**
- **RDP's standard security layer** (RC4 + RSA, the pre-TLS one). It is broken
  and implementing it would mean orrery could be talked down to it.
- **TLS 1.2.** See §5.
- **Anything that writes a key.** `copal fleet login` owns that, and one place
  that makes credentials is the difference between a fleet and a mess.

---

## 8 · Testing, and how each of these is proved without hardware

`rfb.rs` carries a `fake` module — a hand-written RFB 3.8 server — and the seat
is proved against it on a real socket: handshake, decode, input encoding,
modifier release. Every module here does the same.

| module | proved against | proves |
|---|---|---|
| `crypto.rs` | published test vectors | RFC 7748, RFC 8032, RFC 8439, FIPS 180-4, NIST GCM KATs |
| `ssh.rs` | `ssh::fake`, a server on a socket | kex, cert verification, auth, channels, rekey |
| `ssh.rs` | **`sshd` in a container** | what a real server does that a fake one does not |
| `sftp.rs` | OpenSSH's `sftp-server` binary over a pipe | v3 packet shapes, pipelining |
| `tls.rs` | `openssl s_server -tls1_3` | the handshake against an implementation nobody here wrote |
| `rdp.rs` | **hypr-rdp on a node**, and nothing else | there is no fake worth writing for this |

The last row is the honest one. A fake RDP server would be a fake of the
assumptions this document makes, and passing against it would prove that
`rdp.rs` agrees with `rdp.rs`. The seat's own "Still open" entry says the same
thing about `x11vnc` — *"what that does not prove is x11vnc's own choices"* —
and the lesson from that is to get to the real server early rather than to build
a better fake.
