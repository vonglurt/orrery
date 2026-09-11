# One list, in one place, with a number on it

*Locking the guest to public keys, refusing every standard client, and what it
means — and does not mean — for orrery to synchronise the fleet's credentials.*

The ask: a node must not be reachable by a stock client. Not "is hard to
reach", not "warns" — a stock `ssh`, `mstsc`, FreeRDP, Remmina or KRDC must
**fail to negotiate, at the handshake, before authentication is ever offered.**
And the set of things that *are* accepted must be one list in one place.

---

## 1 · Why one place, and why it is code rather than prose

`docs/wire.md` already argued for one algorithm per slot on the client side —
`rfb.rs` asks for Raw and CopyRect and nothing else, and every absence is a
thing that cannot then be got wrong. That is a good property and it is worth
nothing on its own.

**A narrow client in front of a permissive server protects nothing.** If
`sshd` on the node still accepts `aes128-ctr` with `hmac-sha2-256`, then the
console's refusal to speak it is a preference, not a control — anybody else's
client speaks it happily. The narrowness has to be on the node, and the node's
list and the console's list have to be the same list, or they drift and the
node is quietly wider than the console believes.

So the whitelist lives in **`src/profile.rs`**, once, and three things read it:

| reader | what it does with it |
|---|---|
| `ssh.rs`, `tls.rs`, `rdp.rs` | build their offers from it. The console cannot negotiate something the profile does not name, because it has nothing else to offer. |
| `profile::sshd_config()` | renders the lines `copal-prep.sh` writes into the node's policy. `make lint` fails on drift — the same check that already compares the embedded `radbeeper` and `copal_nkeys.py` against their sources. |
| the beacon | the node announces which profile it enforces, so the wall can show one that has drifted. |

A whitelist that exists in a Rust file and again in a shell heredoc is two
whitelists. This one is rendered, not retyped.

---

## 2 · Profile 1

```rust
ssh_kex:              curve25519-sha256
ssh_host_key:         ssh-ed25519-cert-v01@openssh.com
ssh_cipher:           chacha20-poly1305@openssh.com, aes256-gcm@openssh.com
ssh_mac:              (none -- both ciphers carry their own tag)
ssh_pubkey_accepted:  ssh-ed25519-cert-v01@openssh.com

tls_versions:         0x0304, and nothing else
tls_suites:           TLS_CHACHA20_POLY1305_SHA256, TLS_AES_256_GCM_SHA384
tls_groups:           x25519
tls_sig_algs:         ed25519

rdp_protocol:         PROTOCOL_SSL only
rdp_client_cert:      required, issued by the fleet CA
```

Four of those lines are doing the work:

**`cert-v01` on both key lines.** A bare `ssh-ed25519` host key means trust on
first use, and the entire point of the fleet having a certificate authority is
that nobody is ever asked to decide. A bare *user* key means an
`authorized_keys` file is a second place membership lives. Certificates only,
both directions — so enrolment is the only way in and revocation is the
certificate expiring.

**No MAC list at all.** Both ciphers are AEAD, so there is no
encrypt-then-MAC-versus-MAC-then-encrypt question to get wrong, and nothing to
negotiate.

**TLS 1.3 and nothing else.** One version means no downgrade dance, no RSA key
transport, no renegotiation, no 1.2 record layer.

**`rdp_client_cert`.** See §3.

The list is guarded by a test that reads as a policy rather than as data:
`nothing_weak_is_on_any_list` fails the build if `sha1`, `cbc`, `rc4`, `3des`,
`ssh-rsa`, `ecdsa`, `nistp`, `anon` or eighteen other tokens appear anywhere in
it. **Widening the whitelist to something weak is therefore not something that
can happen quietly during a debugging session.**

---

## 3 · What actually stops a standard RDP client

Everything in §2 narrows what may be *spoken*. Exactly one line decides who may
*speak*:

> **The node demands a client certificate issued by the fleet CA.**

mstsc has no fleet certificate. FreeRDP has no fleet certificate. Neither can
be given one, because issuing requires the CA private half, which lives at
`~/.copal/ca/<fleet>_ca` on the operator's Mac and is the one thing that never
goes near a card or a node. A standard client's TLS handshake therefore ends at
`certificate_required`, **before the X.224 negotiation response, before the
MCS connect, before any credential is offered in either direction.**

This is mutual TLS, and it is the strongest control in the whole design —
stronger than anything in `docs/wire.md`, because it is a property of the
*node* rather than a promise about the console. It replaces certificate pinning
(`wire.md` §5), which was recorded there as an interim, and it closes the gap
that section named: RDP now gets the same CA check SSH has.

**It also costs something, and the cost is real.** The fleet CA is an SSH CA —
`ssh-keygen -s` — and X.509 is a different container. Issuing an X.509 client
certificate from the same authority means a small ASN.1 writer somewhere, and
`wire.md` §5 put that out of scope. **This requirement puts it back in.** It is
about 400 lines of DER encoding on top of the parser `tls.rs` needs anyway, it
belongs beside `sign_one()` in `copal-fleet.sh`, and it is now a prerequisite
for phase 8 rather than an improvement on it.

### SSH, for completeness

A stock `ssh` is refused by `PubkeyAcceptedAlgorithms` and
`HostKeyAlgorithms`: it will happily offer `ssh-ed25519`, and the node accepts
only `ssh-ed25519-cert-v01@openssh.com`. A person with the operator
certificate in their agent can still use stock `ssh`, and **that is
deliberate** — the fleet's recovery path must not require the console to be
working. The control is the certificate, not the client binary.

---

## 4 · The guest, locked to public keys

`copal-prep.sh` already has most of this. `COPAL_SSH_PASSWORD_LOGIN='no'`
exists, stage 13 turns password authentication off when it finds an installed
key, and root over SSH is always refused. What changes is that for a **fleet
card** these stop being consequences of a default and become a policy that
cannot be edited back on by accident:

```
AuthenticationMethods publickey          <- the line that makes it true
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
GSSAPIAuthentication no
HostbasedAuthentication no
PermitEmptyPasswords no
PermitRootLogin no
```

`AuthenticationMethods publickey` is the one that matters and the one that is
usually missing. With it, sshd will not complete a login by any other path
**even if a later line, a package upgrade, or somebody's edit turns one of
them back on** — the methods list is checked independently of whether an
individual method is enabled.

Two consequences worth stating rather than discovering:

- **`COPAL_ROOT_PW_HASH` still matters, and still is not a network credential.**
  The password is for the console on the desk and for `doas`. It has never
  been a way in over the network on a keyed build, and on a fleet card it now
  cannot become one.
- **A fleet card with no SSH key installed is refused at build time**, rather
  than built and then unreachable. `copal-answers.sh` already warns; for a
  fleet card this becomes an error, because the alternative is eight boards
  that can only be recovered with a keyboard and a monitor.

---

## 5 · What "orrery synchronises the credentials" means

This is the part that needs its line held carefully, because the obvious
reading of it breaks the rule `wire.md` §1 just finished restating:

> orrery **never creates** a credential, never stores one, never prompts for
> one, never forwards one, and never outlives one.

**Synchronise means distribute and verify. It does not mean mint.**
`copal fleet login` makes operator keys. `sign_one()` issues host certificates.
`copal-answers.sh` generates enrolment tokens. Those three stay exactly where
they are, on the machine with the CA, and orrery calls them — it does not
reimplement them. One place that makes credentials is the difference between a
fleet and a mess.

What orrery adds is the thing that is currently nobody's job: **knowing whether
eight machines agree.**

### The credentials pane

A column per node, from the read model and from `copal fleet ls`:

| reading | comes from | what a bad value means |
|---|---|---|
| host certificate, days left | `facts()` `cert_days` | 90 days from enrolment; under 14 is a chip |
| CA fingerprint | `facts()` | a node trusting **another CA** is the `foreign` state `cmd_ls` already reports, and is an alarm |
| crypto profile | `facts()` `profile` | older than `MIN_ACCEPTED` and the console will not connect |
| enrolment token | the ledger | `unused` on a node that is up means a card that never enrolled |
| RDP client CA | `facts()` | whether the node will demand a certificate, or is still open |

and three actions, each of which is a call to something that already exists:

- **Re-sign** a node whose certificate is short — `copal fleet sign --node ID`.
- **Push the profile** to a node that has drifted — which is a `remote`-style
  verb that re-renders the sshd block from the node's own copy of the profile,
  and is *not* the console sending configuration text. Same discipline as the
  bus: the console sends a **profile number**, the node renders the lines. A
  console that has been tampered with cannot widen an allow-list by sending a
  cleverer file, which is `fleet-m4-backlog.md` §3 D1's rule applied to
  ciphers.
- **Refuse**, loudly, and say which of the five readings is wrong.

**The console never holds the CA private half, and the pane does not offer to.**
Re-sign runs the CLI, which runs on the machine that has it.

---

## 6 · Rotating the profile

The reason the version number exists:

1. Edit `src/profile.rs`. Bump `CURRENT`.
2. `make sync-profile` re-renders the sshd block into `copal-prep.sh`.
   `make lint` fails until this is done, so it cannot be forgotten.
3. Rebuild cards, or push the profile number to enrolled nodes.
4. Raise `MIN_ACCEPTED` to match once every node reports the new number.

Step 4 is the one that closes the rotation, and until it happens the console
will still talk to the old profile. **A fleet mid-rotation — six cards
reflashed, two not yet — is driven for an afternoon by lowering one number,
not by widening a cipher list.** That is the whole design: the escape hatch is
a visible, temporary, single-line diff in a file somebody reads, rather than an
invisible permanent widening inside a list nobody re-reads.

A test enforces even that much: `the_minimum_is_never_quietly_below_the_current_profile`
fails when `MIN_ACCEPTED < CURRENT`, with a message asking for the rotation to
be named in the commit.

---

## 7 · What this does not protect against, stated plainly

- **A stolen operator key.** It is an unencrypted Ed25519 key on the operator's
  Mac, by `cmd_login`'s own design, and the mitigation is that it expires in
  eight hours. Anyone holding it holds the fleet until it does.
- **The CA private half.** Everything here rests on it. It is at
  `~/.copal/ca/<fleet>_ca`, mode 600, on one machine, and nothing in orrery
  reads, copies, or mentions it.
- **A compromised node.** Certificates prove identity, not integrity. A node
  somebody has root on presents a valid host certificate because it is the
  node.
- **Traffic analysis on the LAN.** Invariant 6 says the LAN never reaches the
  internet; it does not say the LAN is trusted.
- **The crypto being correct.** `wire.md` R3 is unchanged: this is
  hand-written, and narrowness reduces the surface without eliminating the
  risk. A narrow list of hand-written primitives is a better bet than a wide
  one, and it is not the same bet as a reviewed library.

None of these are reasons not to do §2 through §6. They are the things that
remain true afterwards, and a document that only listed what it fixed would be
the wrong kind of document.
