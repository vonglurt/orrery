//! SSH-2, by hand -- the transport every other verb was waiting for.
//!
//! WHAT THIS UNLOCKS, WHICH IS THE REASON IT IS THE NEXT PHASE. Terminal,
//! Exchange, Send and Message are all absent from the verb bar because their
//! transport did not exist. This is that transport. `pty.rs` already does the
//! half of Terminal that runs a child on a real terminal; what was missing was
//! the socket where that child lives on another machine.
//!
//! THE SHAPE OF THE NARROWNESS. `profile.rs` names one key exchange, one host
//! key algorithm, one signature algorithm and two ciphers, and this file can
//! offer nothing else -- there is no second code path to fall back to. That is
//! the lockdown working as designed: a console that cannot speak `ssh-rsa`
//! cannot be talked into speaking it.
//!
//! CERTIFICATES, NOT KNOWN_HOSTS. The host key this client accepts is an
//! `ssh-ed25519-cert-v01@openssh.com` certificate signed by the fleet CA, with
//! the node's hostname among its principals and its validity window checked
//! against the clock. There is no trust-on-first-use path and no prompt,
//! because a prompt is a decision the CA already made. `copal-prep.sh` stage 16
//! puts the host certificate on the node; `docs/fleet-control.md` §5 is the
//! other half of this sentence.
//!
//! WHY IT OFFERS ONE CIPHER WHEN THE PROFILE ALLOWS TWO. In SSH the CLIENT's
//! preference wins -- the server takes the first name on the client's list it
//! also has -- so offering ChaCha alone and offering ChaCha first are the same
//! session against any server in this fleet. The difference is what happens
//! against a server that lacks it: offering only ChaCha fails the handshake,
//! which is the correct outcome for a console whose AES runs at half a megabyte
//! a second (`crypto.rs`, and R4 in `docs/wire.md`). The profile is what may be
//! ACCEPTED; the offer is one line of it.
//!
//! WHAT IS DELIBERATELY NOT HERE: agent forwarding, X11 forwarding, port
//! forwarding, keyboard-interactive, password authentication, compression, and
//! every cipher with a block mode. Each is a feature this console has no use
//! for and a surface somebody else has had a CVE in.

use crate::crypto::{self};
use crate::profile;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The version string this client announces. It travels in the exchange hash,
/// so it is a protocol constant rather than a decoration.
pub const VERSION: &str = "SSH-2.0-orrery_0.1";

// Message numbers, named rather than spelled as digits at the call sites.
mod msg {
    pub const DISCONNECT: u8 = 1;
    pub const IGNORE: u8 = 2;
    pub const UNIMPLEMENTED: u8 = 3;
    pub const DEBUG: u8 = 4;
    pub const SERVICE_REQUEST: u8 = 5;
    pub const SERVICE_ACCEPT: u8 = 6;
    pub const EXT_INFO: u8 = 7;
    pub const KEXINIT: u8 = 20;
    pub const NEWKEYS: u8 = 21;
    pub const KEX_ECDH_INIT: u8 = 30;
    pub const KEX_ECDH_REPLY: u8 = 31;
    pub const USERAUTH_REQUEST: u8 = 50;
    pub const USERAUTH_FAILURE: u8 = 51;
    pub const USERAUTH_SUCCESS: u8 = 52;
    pub const USERAUTH_BANNER: u8 = 53;
    pub const GLOBAL_REQUEST: u8 = 80;
    pub const REQUEST_FAILURE: u8 = 82;
    pub const CHANNEL_OPEN: u8 = 90;
    pub const CHANNEL_OPEN_CONFIRMATION: u8 = 91;
    pub const CHANNEL_OPEN_FAILURE: u8 = 92;
    pub const CHANNEL_WINDOW_ADJUST: u8 = 93;
    pub const CHANNEL_DATA: u8 = 94;
    pub const CHANNEL_EXTENDED_DATA: u8 = 95;
    pub const CHANNEL_EOF: u8 = 96;
    pub const CHANNEL_CLOSE: u8 = 97;
    pub const CHANNEL_REQUEST: u8 = 98;
    pub const CHANNEL_SUCCESS: u8 = 99;
    pub const CHANNEL_FAILURE: u8 = 100;
}

// ---------------------------------------------------------------------------
// The wire's own vocabulary
// ---------------------------------------------------------------------------
//
// SSH has four types on the wire -- byte, uint32, string and mpint -- and
// almost every protocol bug in almost every implementation is one of them
// written or read wrong. They get their own pair of types here so that no call
// site ever writes a length by hand.

#[derive(Default, Clone)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    pub fn new() -> Buf {
        Buf(Vec::new())
    }
    pub fn u8(&mut self, v: u8) -> &mut Buf {
        self.0.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Buf {
        self.0.push(v as u8);
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Buf {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Buf {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn raw(&mut self, v: &[u8]) -> &mut Buf {
        self.0.extend_from_slice(v);
        self
    }
    /// A length-prefixed string, which in SSH is also how every blob travels.
    pub fn string(&mut self, v: &[u8]) -> &mut Buf {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    pub fn str(&mut self, v: &str) -> &mut Buf {
        self.string(v.as_bytes())
    }
    pub fn list(&mut self, v: &[&str]) -> &mut Buf {
        self.str(&v.join(","))
    }
    /// A multiple-precision integer: big-endian, no leading zeroes, and a
    /// leading zero byte when the top bit is set BECAUSE MPINT IS SIGNED. The
    /// shared secret K goes into the exchange hash this way, so getting it
    /// wrong produces a hash that differs from the server's roughly half the
    /// time -- an intermittent handshake failure, which is the worst kind.
    pub fn mpint(&mut self, v: &[u8]) -> &mut Buf {
        let first = v.iter().position(|b| *b != 0).unwrap_or(v.len());
        let t = &v[first..];
        if t.is_empty() {
            self.u32(0);
        } else if t[0] & 0x80 != 0 {
            self.u32(t.len() as u32 + 1);
            self.0.push(0);
            self.0.extend_from_slice(t);
        } else {
            self.string(t);
        }
        self
    }
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

pub struct Cur<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Cur<'a> {
        Cur { b, at: 0 }
    }
    fn need(&self, n: usize) -> Result<(), String> {
        if self.at + n > self.b.len() {
            return Err("the packet ended in the middle of a field".into());
        }
        Ok(())
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.at += 1;
        Ok(self.b[self.at - 1])
    }
    pub fn bool(&mut self) -> Result<bool, String> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        self.need(4)?;
        let v = u32::from_be_bytes([
            self.b[self.at],
            self.b[self.at + 1],
            self.b[self.at + 2],
            self.b[self.at + 3],
        ]);
        self.at += 4;
        Ok(v)
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok((self.u32()? as u64) << 32 | self.u32()? as u64)
    }
    pub fn string(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        // A length field is attacker-controlled before anything is
        // authenticated, so it is checked against what actually arrived rather
        // than used to size an allocation.
        self.need(n)?;
        self.at += n;
        Ok(&self.b[self.at - n..self.at])
    }
    pub fn text(&mut self) -> Result<String, String> {
        let s = self.string()?;
        String::from_utf8(s.to_vec()).map_err(|_| "a name was not UTF-8".to_string())
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.at..]
    }
    pub fn done(&self) -> bool {
        self.at >= self.b.len()
    }
}

// ---------------------------------------------------------------------------
// Keys and certificates, in OpenSSH's own formats
// ---------------------------------------------------------------------------

/// The name of the only public-key algorithm on the fleet, and the name of the
/// certificate wrapper around it. Both spellings appear on the wire and they
/// are not interchangeable: the certificate name is what a `publickey` request
/// advertises, the bare name is what the signature inside it is labelled with.
pub const ED25519: &str = "ssh-ed25519";
pub const ED25519_CERT: &str = "ssh-ed25519-cert-v01@openssh.com";

/// An OpenSSH public key blob: the algorithm name, then the key.
pub fn ed25519_blob(pk: &[u8; 32]) -> Vec<u8> {
    let mut b = Buf::new();
    b.str(ED25519).string(pk);
    b.take()
}

/// Read the 32 bytes out of an `ssh-ed25519` blob.
pub fn ed25519_from_blob(blob: &[u8]) -> Result<[u8; 32], String> {
    let mut c = Cur::new(blob);
    if c.text()? != ED25519 {
        return Err("that is not an ssh-ed25519 key".into());
    }
    let k = c.string()?;
    if k.len() != 32 {
        return Err("an ed25519 key is thirty-two bytes".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(k);
    Ok(out)
}

/// What a certificate is for. A host certificate presented for user
/// authentication -- or the reverse -- is a real attack and not a mix-up, so
/// the two are separate values and every check names which one it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertKind {
    User,
    Host,
}

/// A parsed `ssh-ed25519-cert-v01@openssh.com`.
#[derive(Debug, Clone)]
pub struct Cert {
    pub blob: Vec<u8>,
    pub key: [u8; 32],
    pub serial: u64,
    pub kind: CertKind,
    pub key_id: String,
    pub principals: Vec<String>,
    pub valid_after: u64,
    pub valid_before: u64,
    pub extensions: Vec<String>,
    pub ca: [u8; 32],
    /// How much of `blob` the signature covers: everything before it.
    signed_len: usize,
    signature: [u8; 64],
}

impl Cert {
    pub fn parse(blob: &[u8]) -> Result<Cert, String> {
        let mut c = Cur::new(blob);
        if c.text()? != ED25519_CERT {
            return Err("that is not an ed25519 certificate".into());
        }
        let _nonce = c.string()?;
        let key = {
            let k = c.string()?;
            if k.len() != 32 {
                return Err("the certified key is not ed25519".into());
            }
            let mut o = [0u8; 32];
            o.copy_from_slice(k);
            o
        };
        let serial = c.u64()?;
        let kind = match c.u32()? {
            1 => CertKind::User,
            2 => CertKind::Host,
            n => return Err(format!("certificate type {} is not a type", n)),
        };
        let key_id = c.text()?;
        let principals = {
            let inner = c.string()?;
            let mut p = Cur::new(inner);
            let mut out = Vec::new();
            while !p.done() {
                out.push(p.text()?);
            }
            out
        };
        let valid_after = c.u64()?;
        let valid_before = c.u64()?;
        let _crit = c.string()?;
        let extensions = {
            let inner = c.string()?;
            let mut p = Cur::new(inner);
            let mut out = Vec::new();
            while !p.done() {
                out.push(p.text()?);
                // Each extension carries a value, which for the ones the fleet
                // uses is empty. It is read and dropped rather than skipped by
                // arithmetic.
                let _ = p.string()?;
            }
            out
        };
        let _reserved = c.string()?;
        let ca = ed25519_from_blob(c.string()?)?;
        let signed_len = c.at;
        let sigblob = c.string()?;
        if !c.done() {
            return Err("there is something after the certificate".into());
        }
        let mut s = Cur::new(sigblob);
        if s.text()? != ED25519 {
            return Err("the certificate is not signed with ed25519".into());
        }
        let sig = s.string()?;
        if sig.len() != 64 {
            return Err("the certificate's signature is the wrong size".into());
        }
        let mut signature = [0u8; 64];
        signature.copy_from_slice(sig);
        Ok(Cert {
            blob: blob.to_vec(),
            key,
            serial,
            kind,
            key_id,
            principals,
            valid_after,
            valid_before,
            extensions,
            ca,
            signed_len,
            signature,
        })
    }

    /// Every check, in one place, in the order that makes a failure legible.
    ///
    /// THE ORDER IS DELIBERATE. The signature is checked first, because until
    /// it is checked the principals and the dates are attacker-supplied text.
    /// A implementation that reads the principal list first and the signature
    /// second is one refactor away from trusting a name it never verified.
    pub fn check(
        &self,
        cas: &[[u8; 32]],
        kind: CertKind,
        principal: &str,
        now: u64,
    ) -> Result<(), String> {
        if !cas.iter().any(|c| crypto::ct_eq(c, &self.ca)) {
            return Err("the certificate was signed by a CA this fleet does not know".into());
        }
        if !crypto::ed25519_verify(&self.ca, &self.blob[..self.signed_len], &self.signature) {
            return Err("the certificate's signature does not check out".into());
        }
        if self.kind != kind {
            return Err(format!(
                "that is a {:?} certificate and this is a {:?} check",
                self.kind, kind
            ));
        }
        if now < self.valid_after || now > self.valid_before {
            return Err(format!(
                "the certificate for {} is not valid now",
                self.key_id
            ));
        }
        // An empty principal list means "any", which is a thing OpenSSH
        // permits and this fleet does not: `docs/lockdown.md` says every
        // certificate names what it is for.
        if self.principals.is_empty() {
            return Err("the certificate names no principals".into());
        }
        if !self.principals.iter().any(|p| p == principal) {
            return Err(format!(
                "the certificate is for {} and not for {}",
                self.principals.join(","),
                principal
            ));
        }
        Ok(())
    }
}

/// Read an `ssh-ed25519` private key in OpenSSH's own format.
///
/// UNENCRYPTED ONLY, AND THAT IS A CHOICE WITH A REASON. A passphrase on the
/// operator key would be typed into this console, which means this console
/// would have to hold a passphrase prompt, a memory-scrubbing story and a
/// bcrypt KDF. The fleet's answer is a key file readable only by the operator's
/// account on the operator's machine -- the same posture `ssh-agent` has once
/// it is unlocked -- and a CA that can revoke it. A key with a KDF on it is
/// refused loudly rather than half-supported.
pub fn parse_private_key(text: &str) -> Result<([u8; 32], [u8; 32]), String> {
    const BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
    const END: &str = "-----END OPENSSH PRIVATE KEY-----";
    let start = text
        .find(BEGIN)
        .ok_or("that is not an OpenSSH private key")?
        + BEGIN.len();
    let end = text.find(END).ok_or("the private key has no end")?;
    let raw = crypto::b64_decode(&text[start..end])?;

    let magic = b"openssh-key-v1\0";
    if raw.len() < magic.len() || &raw[..magic.len()] != magic {
        return Err("the private key is not openssh-key-v1".into());
    }
    let mut c = Cur::new(&raw[magic.len()..]);
    let cipher = c.text()?;
    let kdf = c.text()?;
    let _kdfopts = c.string()?;
    if cipher != "none" || kdf != "none" {
        return Err("this key has a passphrase on it, and orrery does not ask for one".into());
    }
    if c.u32()? != 1 {
        return Err("a key file with more than one key in it".into());
    }
    let _pub = c.string()?;
    let private = c.string()?;

    let mut p = Cur::new(private);
    let a = p.u32()?;
    let b = p.u32()?;
    if a != b {
        // With no cipher these are always equal; unequal means the file is
        // damaged rather than that a passphrase was wrong.
        return Err("the private key's check words disagree".into());
    }
    if p.text()? != ED25519 {
        return Err("the private key is not ed25519".into());
    }
    let pk = p.string()?;
    let sk = p.string()?;
    if pk.len() != 32 || sk.len() != 64 {
        return Err("the private key is the wrong size".into());
    }
    let mut seed = [0u8; 32];
    let mut public = [0u8; 32];
    seed.copy_from_slice(&sk[..32]);
    public.copy_from_slice(pk);
    // OpenSSH stores seed || public. Recomputing the public half from the seed
    // and comparing is free, and it catches a truncated or swapped file here
    // rather than as an authentication failure nobody can explain.
    if crypto::ed25519_public(&seed) != public {
        return Err("the private key and its public half do not match".into());
    }
    Ok((seed, public))
}

/// Read a one-line `.pub` file: an algorithm name, base64, and a comment.
pub fn parse_public_line(line: &str) -> Result<(String, Vec<u8>), String> {
    let mut parts = line.split_whitespace();
    let algo = parts.next().ok_or("an empty public key line")?.to_string();
    let body = parts.next().ok_or("a public key line with no key on it")?;
    let blob = crypto::b64_decode(body)?;
    let mut c = Cur::new(&blob);
    if c.text()? != algo {
        return Err("the key's name and the line's name disagree".into());
    }
    Ok((algo, blob))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The binary packet protocol
// ---------------------------------------------------------------------------
//
// A packet is a length, a padding length, the payload, the padding and -- once
// keys exist -- an authentication tag. `chacha20-poly1305@openssh.com` splits
// its sixty-four bytes of key material in two: the first thirty-two encrypt the
// payload, the second thirty-two encrypt the four-byte length on its own, so a
// receiver can learn how much to read without having authenticated anything
// yet. The sequence number is the nonce. There is no separate MAC key and no
// MAC negotiation, which is why `profile.rs` lists no MAC algorithms at all.
//
// THE LENGTH IS NEVER TRUSTED BEFORE THE TAG IS. It is decrypted to know how
// much to wait for, and nothing is acted on until Poly1305 has agreed that the
// length and the ciphertext arrived as the peer sent them.

const MAX_PACKET: usize = 256 * 1024;
const CHANNEL_WINDOW: u32 = 2 * 1024 * 1024;
const CHANNEL_MAX: u32 = 32 * 1024;
/// One gigabyte or one hour, whichever comes first -- RFC 4253 §9's advice,
/// and the reason a long RDP session over a forwarded channel does not run all
/// day under one key.
const REKEY_BYTES: u64 = 1 << 30;
const REKEY_SECONDS: u64 = 3600;

struct Cipher {
    /// K_2 -- the payload.
    main: [u8; 32],
    /// K_1 -- the length field, alone.
    header: [u8; 32],
}

impl Cipher {
    fn new(material: &[u8]) -> Cipher {
        let mut main = [0u8; 32];
        let mut header = [0u8; 32];
        main.copy_from_slice(&material[..32]);
        header.copy_from_slice(&material[32..64]);
        Cipher { main, header }
    }

    fn poly_key(&self, nonce: &[u8; 8]) -> [u8; 32] {
        let mut block = [0u8; 64];
        crypto::chacha20_xor64(&self.main, 0, nonce, &mut block);
        let mut k = [0u8; 32];
        k.copy_from_slice(&block[..32]);
        k
    }
}

/// What the client will believe about the other end.
#[derive(Clone)]
pub struct Trust {
    /// The fleet CA public keys. More than one because a rotation has two.
    pub cas: Vec<[u8; 32]>,
    /// The principal the host certificate must carry -- the node's name, not
    /// the address it happened to be reached at.
    pub host: String,
}

/// One connection: a socket, a pair of directions, and what has been agreed.
pub struct Conn {
    s: TcpStream,
    inbuf: Vec<u8>,
    out_seq: u32,
    in_seq: u32,
    tx: Option<Cipher>,
    rx: Option<Cipher>,
    v_c: String,
    v_s: String,
    pub session_id: Vec<u8>,
    pub cert: Option<Cert>,
    trust: Trust,
    bytes: u64,
    last_kex: Instant,
    /// The socket has finished. NOT AN ERROR BY ITSELF: a command that ran to
    /// completion and a node that was unplugged look identical at this layer,
    /// and the difference is whether the packets already in the buffer said
    /// what happened. So the flag is set, the buffer is still parsed, and only
    /// a caller that wanted another packet is told the connection is gone.
    ended: bool,
    /// Set while a key exchange is in flight, so `recv` does not try to start
    /// a second one from inside the first.
    rekeying: bool,
}

impl Conn {
    /// How long a single blocking wait for a packet may take.
    const TIMEOUT: Duration = Duration::from_secs(20);

    pub fn connect(addr: &str, trust: Trust) -> Result<Conn, String> {
        let s = TcpStream::connect(addr).map_err(|e| format!("cannot reach {}: {}", addr, e))?;
        s.set_nodelay(true).ok();
        // A short read timeout rather than a non-blocking socket: every read
        // below is "give me what has arrived", and a timeout says that in one
        // place instead of at every call site.
        s.set_read_timeout(Some(Duration::from_millis(20))).ok();
        Ok(Conn {
            s,
            inbuf: Vec::new(),
            out_seq: 0,
            in_seq: 0,
            tx: None,
            rx: None,
            v_c: VERSION.to_string(),
            v_s: String::new(),
            session_id: Vec::new(),
            cert: None,
            trust,
            bytes: 0,
            last_kex: Instant::now(),
            ended: false,
            rekeying: false,
        })
    }

    fn pump(&mut self) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        let mut buf = [0u8; 16384];
        loop {
            match self.s.read(&mut buf) {
                Ok(0) => {
                    self.ended = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.inbuf.extend_from_slice(&buf[..n]);
                    if n < buf.len() {
                        return Ok(());
                    }
                }
                Err(ref e)
                    if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                {
                    return Ok(())
                }
                // A reset is what a peer that has said everything it intends
                // to say and then closed looks like, and it arrives whether or
                // not the exchange finished happily.
                Err(ref e)
                    if e.kind() == ErrorKind::ConnectionReset
                        || e.kind() == ErrorKind::ConnectionAborted
                        || e.kind() == ErrorKind::BrokenPipe =>
                {
                    self.ended = true;
                    return Ok(());
                }
                Err(e) => return Err(format!("reading from the node: {}", e)),
            }
        }
    }

    /// True once the socket has finished AND everything it delivered has been
    /// taken out of the buffer.
    pub fn ended(&self) -> bool {
        self.ended && self.inbuf.len() < 4
    }

    /// The version exchange. Both lines go into the exchange hash, so they are
    /// kept rather than merely checked.
    fn exchange_versions(&mut self) -> Result<(), String> {
        self.s
            .write_all(format!("{}\r\n", VERSION).as_bytes())
            .map_err(|e| format!("writing the version: {}", e))?;
        let start = Instant::now();
        loop {
            if let Some(pos) = self.inbuf.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.inbuf[..pos]).to_string();
                self.inbuf.drain(..pos + 2);
                // Anything before the version line is a banner, which the
                // protocol allows and this fleet's nodes do not send.
                if !line.starts_with("SSH-2.0-") {
                    if line.starts_with("SSH-") {
                        return Err(format!("the node speaks {}, which is not SSH-2", line));
                    }
                    continue;
                }
                self.v_s = line;
                return Ok(());
            }
            if start.elapsed() > Self::TIMEOUT {
                return Err("the node never said what it was".into());
            }
            self.pump()?;
        }
    }

    /// Write one packet, encrypting it when there are keys.
    pub fn send(&mut self, payload: &[u8]) -> Result<(), String> {
        if payload.len() > MAX_PACKET {
            return Err("that packet is too large to send".into());
        }
        let mut out = Vec::new();
        match &self.tx {
            None => {
                // Before NEWKEYS the length field is part of the block that is
                // padded; afterwards it is not, because it is encrypted
                // separately. Getting this backwards costs one byte of padding
                // and the whole handshake.
                let mut pad = 8 - ((5 + payload.len()) % 8);
                if pad < 4 {
                    pad += 8;
                }
                out.extend_from_slice(&((1 + payload.len() + pad) as u32).to_be_bytes());
                out.push(pad as u8);
                out.extend_from_slice(payload);
                out.extend_from_slice(&vec![0u8; pad]);
            }
            Some(c) => {
                let mut pad = 8 - ((1 + payload.len()) % 8);
                if pad < 4 {
                    pad += 8;
                }
                let len = (1 + payload.len() + pad) as u32;
                let nonce = (self.out_seq as u64).to_be_bytes();

                let mut lenbuf = len.to_be_bytes();
                crypto::chacha20_xor64(&c.header, 0, &nonce, &mut lenbuf);

                let mut body = Vec::with_capacity(len as usize);
                body.push(pad as u8);
                body.extend_from_slice(payload);
                // Random padding, not zeroes: it is inside the authenticated
                // ciphertext either way, but zero padding hands a traffic
                // analyst a known plaintext in every packet.
                body.extend_from_slice(&crypto::random(pad)?);
                crypto::chacha20_xor64(&c.main, 1, &nonce, &mut body);

                out.extend_from_slice(&lenbuf);
                out.extend_from_slice(&body);
                let key = c.poly_key(&nonce);
                out.extend_from_slice(&crypto::Poly1305::tag(&key, &out));
            }
        }
        if let Err(e) = self.s.write_all(&out) {
            if matches!(
                e.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ) {
                self.ended = true;
                return Ok(());
            }
            return Err(format!("writing to the node: {}", e));
        }
        self.out_seq = self.out_seq.wrapping_add(1);
        self.bytes += out.len() as u64;
        Ok(())
    }

    /// Take one packet out of the buffer if a whole one is there.
    fn parse(&mut self) -> Result<Option<Vec<u8>>, String> {
        match &self.rx {
            None => {
                if self.inbuf.len() < 4 {
                    return Ok(None);
                }
                let len = u32::from_be_bytes([
                    self.inbuf[0],
                    self.inbuf[1],
                    self.inbuf[2],
                    self.inbuf[3],
                ]) as usize;
                if len < 8 || len > MAX_PACKET {
                    return Err(format!("a packet claiming to be {} bytes", len));
                }
                if self.inbuf.len() < 4 + len {
                    return Ok(None);
                }
                let pad = self.inbuf[4] as usize;
                if pad + 1 > len {
                    return Err("a packet that is all padding".into());
                }
                let payload = self.inbuf[5..4 + len - pad].to_vec();
                self.inbuf.drain(..4 + len);
                self.in_seq = self.in_seq.wrapping_add(1);
                Ok(Some(payload))
            }
            Some(c) => {
                if self.inbuf.len() < 4 {
                    return Ok(None);
                }
                let nonce = (self.in_seq as u64).to_be_bytes();
                let mut lenbuf = [0u8; 4];
                lenbuf.copy_from_slice(&self.inbuf[..4]);
                crypto::chacha20_xor64(&c.header, 0, &nonce, &mut lenbuf);
                let len = u32::from_be_bytes(lenbuf) as usize;
                if len < 8 || len > MAX_PACKET {
                    return Err(format!("a packet claiming to be {} bytes", len));
                }
                if self.inbuf.len() < 4 + len + 16 {
                    return Ok(None);
                }
                let key = c.poly_key(&nonce);
                let want = crypto::Poly1305::tag(&key, &self.inbuf[..4 + len]);
                if !crypto::ct_eq(&want, &self.inbuf[4 + len..4 + len + 16]) {
                    return Err("a packet arrived that the node did not send".into());
                }
                let mut body = self.inbuf[4..4 + len].to_vec();
                crypto::chacha20_xor64(&c.main, 1, &nonce, &mut body);
                let pad = body[0] as usize;
                if pad + 1 > len {
                    return Err("a packet that is all padding".into());
                }
                let payload = body[1..len - pad].to_vec();
                self.inbuf.drain(..4 + len + 16);
                self.in_seq = self.in_seq.wrapping_add(1);
                self.bytes += (4 + len + 16) as u64;
                Ok(Some(payload))
            }
        }
    }

    /// One packet, waiting up to `TIMEOUT` for it, with the transport's own
    /// housekeeping already dealt with.
    pub fn recv(&mut self) -> Result<Vec<u8>, String> {
        let start = Instant::now();
        loop {
            if let Some(p) = self.parse()? {
                match p.first().copied().unwrap_or(0) {
                    msg::IGNORE | msg::DEBUG | msg::UNIMPLEMENTED | msg::EXT_INFO => continue,
                    msg::DISCONNECT => {
                        let mut c = Cur::new(&p[1..]);
                        let code = c.u32().unwrap_or(0);
                        let why = c.text().unwrap_or_default();
                        return Err(format!("the node hung up ({}): {}", code, why));
                    }
                    msg::GLOBAL_REQUEST => {
                        // Somebody wants a forwarding or a hostkeys-00 update.
                        // The answer is no, politely, because a refusal the
                        // peer is waiting for is a stall if it never comes.
                        let mut c = Cur::new(&p[1..]);
                        let _name = c.text().unwrap_or_default();
                        if c.bool().unwrap_or(false) {
                            let mut b = Buf::new();
                            b.u8(msg::REQUEST_FAILURE);
                            self.send(&b.take())?;
                        }
                        continue;
                    }
                    msg::KEXINIT if !self.rekeying => {
                        // The server asked to rekey. Answer it before anything
                        // else: no other packet type may cross a key exchange.
                        self.kex(Some(p))?;
                        continue;
                    }
                    _ => return Ok(p),
                }
            }
            if self.ended() {
                return Err("the node closed the connection".into());
            }
            if start.elapsed() > Self::TIMEOUT {
                return Err("the node stopped answering".into());
            }
            self.pump()?;
        }
    }

    /// A packet, but only if one is already here. This is what the channel
    /// loop uses, so a pane that is waiting for output does not block the
    /// thread it is drawn on.
    pub fn try_recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.pump()?;
        loop {
            match self.parse()? {
                None => return Ok(None),
                Some(p) => match p.first().copied().unwrap_or(0) {
                    msg::IGNORE | msg::DEBUG | msg::UNIMPLEMENTED | msg::EXT_INFO => continue,
                    msg::DISCONNECT => {
                        let mut c = Cur::new(&p[1..]);
                        let _ = c.u32();
                        return Err(format!(
                            "the node hung up: {}",
                            c.text().unwrap_or_default()
                        ));
                    }
                    msg::KEXINIT if !self.rekeying => {
                        self.kex(Some(p))?;
                        continue;
                    }
                    _ => return Ok(Some(p)),
                },
            }
        }
    }

    fn expect(&mut self, want: u8) -> Result<Vec<u8>, String> {
        let p = self.recv()?;
        let got = p.first().copied().unwrap_or(0);
        if got != want {
            return Err(format!(
                "expected message {} from the node and got {}",
                want, got
            ));
        }
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// The key exchange
// ---------------------------------------------------------------------------

/// The algorithm names this client will offer, built from the profile so that
/// there is no second list to keep in step.
fn offer_ciphers() -> Vec<&'static str> {
    // One name, chosen from the profile rather than written here. See the note
    // at the top of the file about why the offer is narrower than the profile.
    profile::P1
        .ssh_cipher
        .iter()
        .filter(|c| c.starts_with("chacha20-poly1305"))
        .copied()
        .collect()
}

/// The first of our names that the peer also has. THE CLIENT'S ORDER DECIDES,
/// which is the protocol's rule and the reason this iterates ours and searches
/// theirs rather than the other way round.
fn pick(ours: &[&str], theirs: &str) -> Option<String> {
    ours.iter()
        .find(|o| theirs.split(',').any(|t| t == **o))
        .map(|o| o.to_string())
}

impl Conn {
    fn kexinit_payload(&self) -> Result<Vec<u8>, String> {
        let mut b = Buf::new();
        b.u8(msg::KEXINIT);
        b.raw(&crypto::random(16)?);
        b.list(profile::P1.ssh_kex);
        b.list(profile::P1.ssh_host_key);
        let ciphers = offer_ciphers();
        b.list(&ciphers);
        b.list(&ciphers);
        // NO MAC ALGORITHMS, DELIBERATELY. The cipher authenticates, and a MAC
        // name offered beside an AEAD cipher is a name that could only ever be
        // chosen by a peer doing something this client does not support.
        b.list(&[]);
        b.list(&[]);
        b.list(&["none"]);
        b.list(&["none"]);
        b.list(&[]);
        b.list(&[]);
        b.bool(false);
        b.u32(0);
        Ok(b.take())
    }

    /// A key exchange, from either side's initiative.
    ///
    /// `theirs` is the server's KEXINIT when it arrived first -- which happens
    /// on every connection, because a server sends its KEXINIT the moment the
    /// versions are exchanged, and on a rekey the server asked for.
    fn kex(&mut self, theirs: Option<Vec<u8>>) -> Result<(), String> {
        self.rekeying = true;
        let result = self.kex_inner(theirs);
        self.rekeying = false;
        result
    }

    fn kex_inner(&mut self, theirs: Option<Vec<u8>>) -> Result<(), String> {
        let i_c = self.kexinit_payload()?;
        self.send(&i_c)?;
        let i_s = match theirs {
            Some(p) => p,
            None => self.expect(msg::KEXINIT)?,
        };

        // What the server said it can do, in the order the protocol puts them.
        let mut c = Cur::new(&i_s[17..]); // skip the message byte and the cookie
        let their_kex = c.text()?;
        let their_host = c.text()?;
        let their_c2s = c.text()?;
        let their_s2c = c.text()?;

        let kex = pick(profile::P1.ssh_kex, &their_kex)
            .ok_or("the node does not offer curve25519-sha256")?;
        let hostkey = pick(profile::P1.ssh_host_key, &their_host).ok_or(
            "the node does not offer a certificate host key -- a bare host key is refused",
        )?;
        let ciphers = offer_ciphers();
        let cipher = pick(&ciphers, &their_c2s)
            .and_then(|_| pick(&ciphers, &their_s2c))
            .ok_or("the node does not offer chacha20-poly1305")?;
        let _ = (kex, cipher);

        let (secret, q_c) = crypto::x25519_keypair()?;
        let mut b = Buf::new();
        b.u8(msg::KEX_ECDH_INIT).string(&q_c);
        self.send(&b.take())?;

        let reply = self.expect(msg::KEX_ECDH_REPLY)?;
        let mut c = Cur::new(&reply[1..]);
        let k_s = c.string()?.to_vec();
        let q_s = c.string()?;
        let sigblob = c.string()?;
        if q_s.len() != 32 {
            return Err("the node's ephemeral key is the wrong size".into());
        }
        let mut peer = [0u8; 32];
        peer.copy_from_slice(q_s);
        let k = crypto::x25519(&secret, &peer)?;

        // The exchange hash. Everything both sides said, in one order, hashed
        // -- which is what makes the signature below cover the whole
        // negotiation rather than just the ephemeral keys.
        let mut h = Buf::new();
        h.str(&self.v_c)
            .str(&self.v_s)
            .string(&i_c)
            .string(&i_s)
            .string(&k_s)
            .string(&q_c)
            .string(q_s)
            .mpint(&k);
        let exchange = crypto::sha256(&h.take());

        // THE GATE. A certificate from a CA this fleet knows, of the host kind,
        // naming this node, valid now -- and then a signature over the exchange
        // hash by the key that certificate certifies.
        let cert = Cert::parse(&k_s)?;
        if hostkey != ED25519_CERT {
            return Err("the node offered something that is not a certificate".into());
        }
        cert.check(
            &self.trust.cas,
            CertKind::Host,
            &self.trust.host,
            now_secs(),
        )?;
        let mut s = Cur::new(sigblob);
        if s.text()? != ED25519 {
            return Err("the node signed with something other than ed25519".into());
        }
        let sig = s.string()?;
        if sig.len() != 64 {
            return Err("the node's signature is the wrong size".into());
        }
        let mut sig64 = [0u8; 64];
        sig64.copy_from_slice(sig);
        if !crypto::ed25519_verify(&cert.key, &exchange, &sig64) {
            return Err("the node did not prove it holds the certified key".into());
        }

        self.send(&[msg::NEWKEYS])?;
        self.expect(msg::NEWKEYS)?;

        // The session id is the FIRST exchange hash and never changes, even
        // though every later rekey produces a new H. Signatures made during
        // authentication are bound to it, so a rekey that moved it would
        // silently invalidate what was already proved.
        if self.session_id.is_empty() {
            self.session_id = exchange.clone();
        }
        let sid = self.session_id.clone();
        self.tx = Some(Cipher::new(&derive(&k, &exchange, b'C', &sid, 64)));
        self.rx = Some(Cipher::new(&derive(&k, &exchange, b'D', &sid, 64)));
        self.cert = Some(cert);
        self.bytes = 0;
        self.last_kex = Instant::now();
        Ok(())
    }

    /// Start a key exchange if this connection has run long enough or moved
    /// enough bytes to want one. Called from the channel loop, which is the
    /// only place a rekey can begin without cutting a packet in half.
    pub fn rekey_if_needed(&mut self) -> Result<(), String> {
        if self.rekeying || self.session_id.is_empty() {
            return Ok(());
        }
        if self.bytes < REKEY_BYTES && self.last_kex.elapsed().as_secs() < REKEY_SECONDS {
            return Ok(());
        }
        self.kex(None)
    }
}

/// RFC 4253 §7.2: HASH(K || H || letter || session_id), extended by hashing
/// what has been produced so far until there is enough.
fn derive(k: &[u8; 32], h: &[u8], letter: u8, session_id: &[u8], need: usize) -> Vec<u8> {
    let mut base = Buf::new();
    base.mpint(k).raw(h);
    let base = base.take();

    let mut first = base.clone();
    first.push(letter);
    first.extend_from_slice(session_id);
    let mut out = crypto::sha256(&first);
    while out.len() < need {
        let mut more = base.clone();
        more.extend_from_slice(&out);
        out.extend_from_slice(&crypto::sha256(&more));
    }
    out.truncate(need);
    out
}

// ---------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------

/// Everything needed to open a session on one node.
#[derive(Clone)]
pub struct Dial {
    /// Where to connect -- an address, which may be an IP.
    pub addr: String,
    /// Who the node is, which is what its certificate must say. KEPT APART
    /// FROM THE ADDRESS ON PURPOSE: reaching 10.0.0.7 and being satisfied that
    /// the certificate says 10.0.0.7 proves nothing about which machine
    /// answered, because whoever answered chose the address.
    pub host: String,
    pub user: String,
    /// The operator's private key, and the certificate the CA issued for it.
    pub seed: [u8; 32],
    pub cert: Vec<u8>,
    pub cas: Vec<[u8; 32]>,
}

impl Dial {
    /// The dial for one node of one fleet, from the paths the CA and
    /// `copal-prep.sh` actually use.
    ///
    /// THE ADDRESS AND THE NAME COME FROM DIFFERENT PLACES ON PURPOSE. The
    /// address is whatever the beacon said, which is a hint about where to
    /// knock; the name is the node's identity, which is what the certificate
    /// has to agree with. A node that answers at another node's address gets
    /// exactly as far as the handshake.
    pub fn for_node(
        home: &std::path::Path,
        fleet: &str,
        node: &str,
        address: &str,
        user: &str,
    ) -> Result<Dial, String> {
        let addr = if address.is_empty() {
            // mDNS is what the fleet already uses to find itself; a node with
            // no address in the read model is one the wall has not heard from,
            // and this is the guess the operator would have typed.
            format!("{}.local:22", node)
        } else if address.contains(':') {
            address.to_string()
        } else {
            format!("{}:22", address)
        };
        Dial::from_files(
            &addr,
            node,
            user,
            &home.join("fleets").join(fleet).join("operator"),
            &home.join("fleets").join(fleet).join("operator-cert.pub"),
            &[home.join("ca").join(format!("{}_ca.pub", fleet))],
        )
    }

    /// Build a dial from the files `copal-prep.sh` and the CA put on disk:
    /// `~/.copal/fleets/<fleet>/operator`, its `-cert.pub`, and the fleet CA's
    /// public key. Nothing here is a secret the console invents -- every one of
    /// these was issued somewhere else, which is the whole of what "orrery
    /// synchronises credentials" is allowed to mean.
    pub fn from_files(
        addr: &str,
        host: &str,
        user: &str,
        key: &std::path::Path,
        cert: &std::path::Path,
        cas: &[std::path::PathBuf],
    ) -> Result<Dial, String> {
        let read = |p: &std::path::Path| {
            std::fs::read_to_string(p).map_err(|e| format!("{}: {}", p.display(), e))
        };
        let (seed, _) = parse_private_key(&read(key)?)?;
        let (algo, cert_blob) = parse_public_line(&read(cert)?)?;
        if algo != ED25519_CERT {
            return Err(format!(
                "{} is a {} and the fleet authenticates with certificates",
                cert.display(),
                algo
            ));
        }
        // The certificate must certify the key beside it. Checked here rather
        // than discovered as an authentication failure with no explanation.
        let parsed = Cert::parse(&cert_blob)?;
        if parsed.key != crypto::ed25519_public(&seed) {
            return Err("that certificate is not for that private key".into());
        }
        let mut keys = Vec::new();
        for p in cas {
            let (_, blob) = parse_public_line(&read(p)?)?;
            keys.push(ed25519_from_blob(&blob)?);
        }
        if keys.is_empty() {
            return Err("no certificate authority to check the node against".into());
        }
        Ok(Dial {
            addr: addr.to_string(),
            host: host.to_string(),
            user: user.to_string(),
            seed,
            cert: cert_blob,
            cas: keys,
        })
    }
}

impl Conn {
    /// Version exchange, key exchange, and the certificate check -- everything
    /// before anyone has said who they are.
    pub fn open(d: &Dial) -> Result<Conn, String> {
        let trust = Trust {
            cas: d.cas.clone(),
            host: d.host.clone(),
        };
        let mut c = Conn::connect(&d.addr, trust)?;
        c.exchange_versions()?;
        c.kex(None)?;
        Ok(c)
    }

    /// Authenticate with the operator's certificate.
    ///
    /// ONE METHOD AND NO FALLBACK. If this fails there is nothing else to try:
    /// the node's sshd is configured with `AuthenticationMethods publickey` and
    /// this console has no password to offer even if it were asked.
    pub fn authenticate(&mut self, d: &Dial) -> Result<(), String> {
        let mut b = Buf::new();
        b.u8(msg::SERVICE_REQUEST).str("ssh-userauth");
        self.send(&b.take())?;
        self.expect(msg::SERVICE_ACCEPT)?;

        // The request, up to but not including the signature. This exact byte
        // string is what gets signed, with the session id in front of it --
        // which is what stops a signature from one session being replayed into
        // another.
        let mut req = Buf::new();
        req.u8(msg::USERAUTH_REQUEST)
            .str(&d.user)
            .str("ssh-connection")
            .str("publickey")
            .bool(true)
            .str(ED25519_CERT)
            .string(&d.cert);
        let req = req.take();

        let mut signed = Buf::new();
        signed.string(&self.session_id).raw(&req);
        let sig = crypto::ed25519_sign(&d.seed, &signed.take());

        let mut sigblob = Buf::new();
        // The signature is labelled with the BARE algorithm name even though
        // the key was offered under the certificate name. OpenSSH is strict
        // about this and rejects the request outright if the two are swapped.
        sigblob.str(ED25519).string(&sig);

        let mut full = Buf::new();
        full.raw(&req).string(&sigblob.take());
        self.send(&full.take())?;

        loop {
            let p = self.recv()?;
            match p.first().copied().unwrap_or(0) {
                msg::USERAUTH_SUCCESS => return Ok(()),
                msg::USERAUTH_BANNER => continue,
                msg::USERAUTH_FAILURE => {
                    let mut c = Cur::new(&p[1..]);
                    let methods = c.text().unwrap_or_default();
                    return Err(format!(
                        "the node refused the operator certificate (it wants {})",
                        if methods.is_empty() { "nothing" } else { &methods }
                    ));
                }
                n => return Err(format!("message {} during authentication", n)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// One channel, which is all this console ever wants
// ---------------------------------------------------------------------------
//
// A real SSH client multiplexes: shells, forwards and subsystems all at once
// down one connection. This one opens a single session channel per connection
// and says so, because the console's shape is one verb against one node at a
// time and a multiplexer nobody uses is a state machine nobody tests. `sftp.rs`
// will want a second channel type on the same connection, and that is the
// phase to generalise this -- not before.

/// A session on a node: the same surface `Pty` offers, so the Terminal pane
/// does not care which one it is holding.
pub struct Session {
    conn: Conn,
    local: u32,
    remote: u32,
    /// How much more the node will accept from us before it adjusts.
    window_out: u32,
    max_out: u32,
    /// How much more we have told the node it may send.
    window_in: u32,
    out: Vec<u8>,
    eof: bool,
    closed: bool,
    status: Option<i32>,
}

impl Session {
    /// Connect, authenticate, and open a session channel.
    pub fn open(d: &Dial) -> Result<Session, String> {
        let mut conn = Conn::open(d)?;
        conn.authenticate(d)?;
        let local = 0;
        let mut b = Buf::new();
        b.u8(msg::CHANNEL_OPEN)
            .str("session")
            .u32(local)
            .u32(CHANNEL_WINDOW)
            .u32(CHANNEL_MAX);
        conn.send(&b.take())?;

        let p = conn.recv()?;
        match p.first().copied().unwrap_or(0) {
            msg::CHANNEL_OPEN_CONFIRMATION => {
                let mut c = Cur::new(&p[1..]);
                let _mine = c.u32()?;
                let remote = c.u32()?;
                let window_out = c.u32()?;
                let max_out = c.u32()?.min(CHANNEL_MAX);
                Ok(Session {
                    conn,
                    local,
                    remote,
                    window_out,
                    max_out,
                    window_in: CHANNEL_WINDOW,
                    out: Vec::new(),
                    eof: false,
                    closed: false,
                    status: None,
                })
            }
            msg::CHANNEL_OPEN_FAILURE => {
                let mut c = Cur::new(&p[1..]);
                let _mine = c.u32()?;
                let code = c.u32()?;
                let why = c.text().unwrap_or_default();
                Err(format!("the node refused a session ({}): {}", code, why))
            }
            n => Err(format!("message {} when opening a session", n)),
        }
    }

    /// The node's host certificate, once the handshake has accepted it. The
    /// console shows the key id, because "which certificate let me in" is the
    /// question an operator actually asks.
    pub fn cert(&self) -> Option<&Cert> {
        self.conn.cert.as_ref()
    }

    fn request(&mut self, name: &str, args: &[u8], want_reply: bool) -> Result<(), String> {
        let mut b = Buf::new();
        b.u8(msg::CHANNEL_REQUEST)
            .u32(self.remote)
            .str(name)
            .bool(want_reply)
            .raw(args);
        self.conn.send(&b.take())?;
        if !want_reply {
            return Ok(());
        }
        loop {
            let p = self.conn.recv()?;
            match p.first().copied().unwrap_or(0) {
                msg::CHANNEL_SUCCESS => return Ok(()),
                msg::CHANNEL_FAILURE => return Err(format!("the node refused {}", name)),
                _ => {
                    // Data can arrive before the reply to a request; it is
                    // kept rather than dropped.
                    self.absorb(&p)?;
                }
            }
        }
    }

    /// Ask for a terminal. This is what makes the far end behave the way
    /// `pty.rs` makes the near end behave: prompts, line editing, and a shell
    /// that knows it is talking to a person.
    pub fn pty(&mut self, term: &str, cols: u32, rows: u32) -> Result<(), String> {
        let mut modes = Buf::new();
        // TTY_OP_END alone: the defaults the node's shell picks are the ones a
        // person sitting at the node would get, and an opinion here would be
        // this console's opinion rather than the fleet's.
        modes.u8(0);
        let mut a = Buf::new();
        a.str(term)
            .u32(cols)
            .u32(rows)
            .u32(0)
            .u32(0)
            .string(&modes.take());
        self.request("pty-req", &a.take(), true)
    }

    /// Tell the far end the pane was resized.
    pub fn resize(&mut self, cols: u32, rows: u32) -> Result<(), String> {
        let mut a = Buf::new();
        a.u32(cols).u32(rows).u32(0).u32(0);
        self.request("window-change", &a.take(), false)
    }

    pub fn shell(&mut self) -> Result<(), String> {
        self.request("shell", &[], true)
    }

    /// Run one command rather than a shell. This is the verb that runs the
    /// same thing on every node.
    pub fn exec(&mut self, command: &str) -> Result<(), String> {
        let mut a = Buf::new();
        a.str(command);
        self.request("exec", &a.take(), true)
    }

    /// File in one packet that belongs to this channel.
    fn absorb(&mut self, p: &[u8]) -> Result<(), String> {
        let mut c = Cur::new(&p[1..]);
        match p.first().copied().unwrap_or(0) {
            msg::CHANNEL_DATA => {
                let _ch = c.u32()?;
                let data = c.string()?;
                self.out.extend_from_slice(data);
                self.consumed(data.len() as u32)?;
            }
            msg::CHANNEL_EXTENDED_DATA => {
                let _ch = c.u32()?;
                let _kind = c.u32()?;
                let data = c.string()?;
                // Stderr goes into the same stream as stdout. A terminal pane
                // shows one thing; splitting them would mean a second pane
                // nobody asked for, and `exec` callers want the error text
                // beside the output that failed.
                self.out.extend_from_slice(data);
                self.consumed(data.len() as u32)?;
            }
            msg::CHANNEL_WINDOW_ADJUST => {
                let _ch = c.u32()?;
                let more = c.u32()?;
                self.window_out = self.window_out.saturating_add(more);
            }
            msg::CHANNEL_EOF => self.eof = true,
            msg::CHANNEL_CLOSE => {
                if !self.closed {
                    self.closed = true;
                    let mut b = Buf::new();
                    b.u8(msg::CHANNEL_CLOSE).u32(self.remote);
                    let _ = self.conn.send(&b.take());
                }
            }
            msg::CHANNEL_REQUEST => {
                let _ch = c.u32()?;
                let name = c.text()?;
                let want_reply = c.bool()?;
                if name == "exit-status" {
                    self.status = Some(c.u32()? as i32);
                } else if name == "exit-signal" {
                    // A command killed by a signal has no exit status, and
                    // reporting nothing would leave a caller waiting forever.
                    let sig = c.text().unwrap_or_default();
                    self.status = Some(-1);
                    self.out
                        .extend_from_slice(format!("\r\n[killed by SIG{}]\r\n", sig).as_bytes());
                }
                if want_reply {
                    let mut b = Buf::new();
                    b.u8(msg::CHANNEL_FAILURE).u32(self.remote);
                    self.conn.send(&b.take())?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Give the node back the window the data it sent used up.
    fn consumed(&mut self, n: u32) -> Result<(), String> {
        self.window_in = self.window_in.saturating_sub(n);
        if self.window_in < CHANNEL_WINDOW / 2 {
            let add = CHANNEL_WINDOW - self.window_in;
            let mut b = Buf::new();
            b.u8(msg::CHANNEL_WINDOW_ADJUST).u32(self.remote).u32(add);
            self.conn.send(&b.take())?;
            self.window_in = CHANNEL_WINDOW;
        }
        Ok(())
    }

    /// Read whatever has arrived. Never blocks -- the same contract `Pty::read`
    /// has, which is what lets one pane hold either.
    pub fn read(&mut self) -> Result<Vec<u8>, String> {
        self.conn.rekey_if_needed()?;
        while let Some(p) = self.conn.try_recv()? {
            self.absorb(&p)?;
        }
        if self.conn.ended() {
            // The far end is gone. Whatever it managed to say is already in
            // `out` and is handed back; what it did not say is an exit status
            // this session will report as unknown rather than as zero.
            self.closed = true;
        }
        Ok(std::mem::take(&mut self.out))
    }

    /// Send what the operator typed.
    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        if self.closed {
            return Err("that session has ended".into());
        }
        for chunk in data.chunks(self.max_out as usize) {
            // A window this small means the far end has stopped reading. Wait
            // for it rather than overrunning it, which the protocol treats as
            // a fatal error rather than as backpressure.
            let start = Instant::now();
            while self.window_out < chunk.len() as u32 {
                while let Some(p) = self.conn.try_recv()? {
                    self.absorb(&p)?;
                }
                if start.elapsed() > Duration::from_secs(20) {
                    return Err("the node stopped reading".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut b = Buf::new();
            b.u8(msg::CHANNEL_DATA).u32(self.remote).string(chunk);
            self.conn.send(&b.take())?;
            self.window_out -= chunk.len() as u32;
        }
        Ok(())
    }

    /// The exit status, once the far end has said what it was.
    pub fn finished(&self) -> Option<i32> {
        if self.closed || self.eof {
            return Some(self.status.unwrap_or(0));
        }
        self.status
    }

    /// Say we are done sending, which is how a remote command reading stdin
    /// learns there is no more.
    pub fn eof(&mut self) -> Result<(), String> {
        let mut b = Buf::new();
        b.u8(msg::CHANNEL_EOF).u32(self.remote);
        self.conn.send(&b.take())
    }

    pub fn close(&mut self) {
        if !self.closed {
            let mut b = Buf::new();
            b.u8(msg::CHANNEL_CLOSE).u32(self.remote);
            let _ = self.conn.send(&b.take());
            self.closed = true;
        }
    }

    /// Run one command and wait for it: the whole of the "same command on
    /// every node" verb, in one call.
    pub fn run(d: &Dial, command: &str, timeout: Duration) -> Result<(String, i32), String> {
        let mut s = Session::open(d)?;
        s.exec(command)?;
        s.eof()?;
        let start = Instant::now();
        let mut out = Vec::new();
        loop {
            out.extend_from_slice(&s.read()?);
            if let Some(code) = s.finished() {
                // One more read: the exit status and the last of the output
                // can arrive in either order.
                out.extend_from_slice(&s.read()?);
                s.close();
                return Ok((String::from_utf8_lossy(&out).to_string(), code));
            }
            if start.elapsed() > timeout {
                s.close();
                return Err(format!("{} did not finish in time", command));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// A server to prove it against
// ---------------------------------------------------------------------------
//
// `rfb.rs` carries a hand-written RFB server for the same reason this carries a
// hand-written SSH one: a client tested only against itself is tested against
// its own misunderstandings. This one speaks the other side of every exchange
// above, and -- more usefully -- it can be told to lie. Each way a certificate
// can be wrong is a constructor here and a test below.
//
// WHAT IT DOES NOT PROVE is what a real `sshd` does that this does not: option
// parsing, `ForceCommand`, principals files, and the dozen places OpenSSH is
// stricter than the RFC. `tools/ssh-check.sh` runs the same tests against a
// real `sshd` in a container, and that is the row in `docs/plan.md` that
// matters.

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// How the fake should behave -- including every way it can be wrong.
    #[derive(Clone)]
    pub struct Opts {
        pub principal: String,
        pub kind: CertKind,
        pub valid_from: i64,
        pub valid_to: i64,
        /// Sign the host certificate with a CA the client has never heard of.
        pub foreign_ca: bool,
        /// Sign the exchange hash with a key the certificate does not certify.
        pub wrong_signer: bool,
        pub user: String,
    }

    impl Default for Opts {
        fn default() -> Opts {
            Opts {
                principal: "museum-01".into(),
                kind: CertKind::Host,
                valid_from: -3600,
                valid_to: 3600,
                foreign_ca: false,
                wrong_signer: false,
                user: "copal".into(),
            }
        }
    }

    /// What the server saw, so a test can assert on the client's behaviour
    /// rather than only on its output.
    #[derive(Default, Debug)]
    pub struct Log {
        pub user: String,
        pub auth_algo: String,
        pub auth_ok: bool,
        pub requests: Vec<String>,
        pub command: String,
        pub term: String,
        pub cols: u32,
        pub typed: Vec<u8>,
    }

    pub struct Fixture {
        pub dial: Dial,
        pub handle: JoinHandle<Result<Log, String>>,
    }

    /// Build an OpenSSH certificate. Only the fake mints certificates: the
    /// console distributes and verifies them and never issues one, which is
    /// the line `docs/lockdown.md` draws around the words "synchronise
    /// credentials".
    pub fn make_cert(
        ca_seed: &[u8; 32],
        key: &[u8; 32],
        kind: CertKind,
        principals: &[&str],
        after: u64,
        before: u64,
        key_id: &str,
    ) -> Vec<u8> {
        let mut ps = Buf::new();
        for p in principals {
            ps.str(p);
        }
        let mut b = Buf::new();
        b.str(ED25519_CERT)
            .string(&crypto::random(32).unwrap())
            .string(key)
            .u64(1)
            .u32(match kind {
                CertKind::User => 1,
                CertKind::Host => 2,
            })
            .str(key_id)
            .string(&ps.take())
            .u64(after)
            .u64(before)
            .string(&[])
            .string(&[])
            .string(&[])
            .string(&ed25519_blob(&crypto::ed25519_public(ca_seed)));
        let body = b.take();
        let sig = crypto::ed25519_sign(ca_seed, &body);
        let mut sb = Buf::new();
        sb.str(ED25519).string(&sig);
        let mut out = body;
        let mut tail = Buf::new();
        tail.string(&sb.take());
        out.extend_from_slice(&tail.take());
        out
    }

    /// The server's half of the packet protocol.
    struct Wire {
        s: TcpStream,
        inbuf: Vec<u8>,
        out_seq: u32,
        in_seq: u32,
        tx: Option<Cipher>,
        rx: Option<Cipher>,
    }

    impl Wire {
        fn send(&mut self, payload: &[u8]) -> Result<(), String> {
            let mut out = Vec::new();
            match &self.tx {
                None => {
                    let mut pad = 8 - ((5 + payload.len()) % 8);
                    if pad < 4 {
                        pad += 8;
                    }
                    out.extend_from_slice(&((1 + payload.len() + pad) as u32).to_be_bytes());
                    out.push(pad as u8);
                    out.extend_from_slice(payload);
                    out.extend_from_slice(&vec![0u8; pad]);
                }
                Some(c) => {
                    let mut pad = 8 - ((1 + payload.len()) % 8);
                    if pad < 4 {
                        pad += 8;
                    }
                    let len = (1 + payload.len() + pad) as u32;
                    let nonce = (self.out_seq as u64).to_be_bytes();
                    let mut lenbuf = len.to_be_bytes();
                    crypto::chacha20_xor64(&c.header, 0, &nonce, &mut lenbuf);
                    let mut body = vec![pad as u8];
                    body.extend_from_slice(payload);
                    body.extend_from_slice(&vec![7u8; pad]);
                    crypto::chacha20_xor64(&c.main, 1, &nonce, &mut body);
                    out.extend_from_slice(&lenbuf);
                    out.extend_from_slice(&body);
                    let key = c.poly_key(&nonce);
                    out.extend_from_slice(&crypto::Poly1305::tag(&key, &out));
                }
            }
            self.s.write_all(&out).map_err(|e| e.to_string())?;
            self.out_seq = self.out_seq.wrapping_add(1);
            Ok(())
        }

        fn recv(&mut self) -> Result<Vec<u8>, String> {
            let start = Instant::now();
            loop {
                if let Some(p) = self.parse()? {
                    if matches!(p[0], msg::IGNORE | msg::DEBUG) {
                        continue;
                    }
                    return Ok(p);
                }
                if start.elapsed() > Duration::from_secs(20) {
                    return Err("the client stopped talking".into());
                }
                let mut buf = [0u8; 16384];
                match self.s.read(&mut buf) {
                    Ok(0) => return Err("the client hung up".into()),
                    Ok(n) => self.inbuf.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == ErrorKind::WouldBlock
                            || e.kind() == ErrorKind::TimedOut => {}
                    Err(e) => return Err(e.to_string()),
                }
            }
        }

        fn parse(&mut self) -> Result<Option<Vec<u8>>, String> {
            match &self.rx {
                None => {
                    if self.inbuf.len() < 4 {
                        return Ok(None);
                    }
                    let len = u32::from_be_bytes([
                        self.inbuf[0],
                        self.inbuf[1],
                        self.inbuf[2],
                        self.inbuf[3],
                    ]) as usize;
                    if self.inbuf.len() < 4 + len {
                        return Ok(None);
                    }
                    let pad = self.inbuf[4] as usize;
                    let p = self.inbuf[5..4 + len - pad].to_vec();
                    self.inbuf.drain(..4 + len);
                    self.in_seq = self.in_seq.wrapping_add(1);
                    Ok(Some(p))
                }
                Some(c) => {
                    if self.inbuf.len() < 4 {
                        return Ok(None);
                    }
                    let nonce = (self.in_seq as u64).to_be_bytes();
                    let mut lenbuf = [0u8; 4];
                    lenbuf.copy_from_slice(&self.inbuf[..4]);
                    crypto::chacha20_xor64(&c.header, 0, &nonce, &mut lenbuf);
                    let len = u32::from_be_bytes(lenbuf) as usize;
                    if len > MAX_PACKET {
                        return Err("a packet claiming to be enormous".into());
                    }
                    if self.inbuf.len() < 4 + len + 16 {
                        return Ok(None);
                    }
                    let key = c.poly_key(&nonce);
                    let want = crypto::Poly1305::tag(&key, &self.inbuf[..4 + len]);
                    if !crypto::ct_eq(&want, &self.inbuf[4 + len..4 + len + 16]) {
                        return Err("the client's packet did not authenticate".into());
                    }
                    let mut body = self.inbuf[4..4 + len].to_vec();
                    crypto::chacha20_xor64(&c.main, 1, &nonce, &mut body);
                    let pad = body[0] as usize;
                    let p = body[1..len - pad].to_vec();
                    self.inbuf.drain(..4 + len + 16);
                    self.in_seq = self.in_seq.wrapping_add(1);
                    Ok(Some(p))
                }
            }
        }
    }

    /// Serve exactly one client, then stop.
    pub fn serve(opts: Opts) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let (ca_seed, ca_pub) = crypto::ed25519_keypair().unwrap();
        let (host_seed, host_pub) = crypto::ed25519_keypair().unwrap();
        let (op_seed, op_pub) = crypto::ed25519_keypair().unwrap();
        let (other_seed, _) = crypto::ed25519_keypair().unwrap();

        let now = now_secs() as i64;
        let signing_ca = if opts.foreign_ca { other_seed } else { ca_seed };
        let host_cert = make_cert(
            &signing_ca,
            &host_pub,
            opts.kind,
            &[&opts.principal],
            (now + opts.valid_from) as u64,
            (now + opts.valid_to) as u64,
            "museum-01 host key",
        );
        let op_cert = make_cert(
            &ca_seed,
            &op_pub,
            CertKind::User,
            &["fleet-operator", "fleet-human"],
            (now - 3600) as u64,
            (now + 3600) as u64,
            "operator",
        );

        let dial = Dial {
            addr: format!("127.0.0.1:{}", port),
            host: "museum-01".into(),
            user: opts.user.clone(),
            seed: op_seed,
            cert: op_cert,
            cas: vec![ca_pub],
        };

        let sign_seed = if opts.wrong_signer { other_seed } else { host_seed };
        let handle = std::thread::spawn(move || {
            let r = run(listener, opts, host_cert, sign_seed, ca_pub);
            // A fake that fails silently makes the client look broken. With
            // `--nocapture` this line is usually the whole debugging session.
            if let Err(e) = &r {
                eprintln!("the fake server gave up: {}", e);
            }
            r
        });
        Fixture { dial, handle }
    }

    fn run(
        listener: TcpListener,
        _opts: Opts,
        host_cert: Vec<u8>,
        sign_seed: [u8; 32],
        ca_pub: [u8; 32],
    ) -> Result<Log, String> {
        let (s, _) = listener.accept().map_err(|e| e.to_string())?;
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(Duration::from_millis(20))).ok();
        let mut w = Wire {
            s,
            inbuf: Vec::new(),
            out_seq: 0,
            in_seq: 0,
            tx: None,
            rx: None,
        };
        let mut log = Log::default();

        // Versions.
        let v_s = "SSH-2.0-orrery_fake";
        w.s.write_all(format!("{}\r\n", v_s).as_bytes())
            .map_err(|e| e.to_string())?;
        let start = Instant::now();
        let v_c = loop {
            if let Some(pos) = w.inbuf.windows(2).position(|x| x == b"\r\n") {
                let line = String::from_utf8_lossy(&w.inbuf[..pos]).to_string();
                w.inbuf.drain(..pos + 2);
                break line;
            }
            if start.elapsed() > Duration::from_secs(10) {
                return Err("no version from the client".into());
            }
            let mut buf = [0u8; 1024];
            match w.s.read(&mut buf) {
                Ok(0) => return Err("the client hung up before saying hello".into()),
                Ok(n) => w.inbuf.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
        };

        // KEXINIT, ours then theirs.
        let mut b = Buf::new();
        b.u8(msg::KEXINIT)
            .raw(&crypto::random(16).unwrap())
            .list(profile::P1.ssh_kex)
            .list(profile::P1.ssh_host_key)
            .list(&["chacha20-poly1305@openssh.com"])
            .list(&["chacha20-poly1305@openssh.com"])
            .list(&[])
            .list(&[])
            .list(&["none"])
            .list(&["none"])
            .list(&[])
            .list(&[])
            .bool(false)
            .u32(0);
        let i_s = b.take();
        w.send(&i_s)?;
        let i_c = w.recv()?;
        if i_c[0] != msg::KEXINIT {
            return Err("the client did not begin with KEXINIT".into());
        }

        // The exchange.
        let init = w.recv()?;
        if init[0] != msg::KEX_ECDH_INIT {
            return Err("the client did not send an ECDH init".into());
        }
        let mut c = Cur::new(&init[1..]);
        let q_c = c.string()?.to_vec();
        let (secret, q_s) = crypto::x25519_keypair()?;
        let mut peer = [0u8; 32];
        peer.copy_from_slice(&q_c);
        let k = crypto::x25519(&secret, &peer)?;

        let mut h = Buf::new();
        h.str(&v_c)
            .str(v_s)
            .string(&i_c)
            .string(&i_s)
            .string(&host_cert)
            .string(&q_c)
            .string(&q_s)
            .mpint(&k);
        let exchange = crypto::sha256(&h.take());
        let sig = crypto::ed25519_sign(&sign_seed, &exchange);
        let mut sb = Buf::new();
        sb.str(ED25519).string(&sig);
        let mut reply = Buf::new();
        reply
            .u8(msg::KEX_ECDH_REPLY)
            .string(&host_cert)
            .string(&q_s)
            .string(&sb.take());
        w.send(&reply.take())?;

        w.send(&[msg::NEWKEYS])?;
        let p = w.recv()?;
        if p[0] != msg::NEWKEYS {
            return Err("the client did not answer NEWKEYS".into());
        }
        let sid = exchange.clone();
        w.rx = Some(Cipher::new(&derive(&k, &exchange, b'C', &sid, 64)));
        w.tx = Some(Cipher::new(&derive(&k, &exchange, b'D', &sid, 64)));

        // Authentication.
        let p = w.recv()?;
        if p[0] != msg::SERVICE_REQUEST {
            return Err("the client asked for no service".into());
        }
        let mut b = Buf::new();
        b.u8(msg::SERVICE_ACCEPT).str("ssh-userauth");
        w.send(&b.take())?;

        let p = w.recv()?;
        if p[0] != msg::USERAUTH_REQUEST {
            return Err("that was not an authentication request".into());
        }
        {
            let mut c = Cur::new(&p[1..]);
            log.user = c.text()?;
            let _service = c.text()?;
            let method = c.text()?;
            let _has_sig = c.bool()?;
            log.auth_algo = c.text()?;
            let blob = c.string()?.to_vec();
            let signed_to = 1 + c.at;
            let sigblob = c.string()?;

            // THE FAKE CHECKS THE CLIENT'S SIGNATURE PROPERLY. A fake that
            // accepted anything would let a broken signing path pass every
            // test in this file.
            let cert = Cert::parse(&blob)?;
            cert.check(&[ca_pub], CertKind::User, "fleet-operator", now_secs())?;
            let mut sc = Cur::new(sigblob);
            let algo = sc.text()?;
            let raw = sc.string()?;
            let mut sig64 = [0u8; 64];
            if raw.len() == 64 {
                sig64.copy_from_slice(raw);
            }
            let mut signed = Buf::new();
            signed.string(&sid).raw(&p[..signed_to]);
            log.auth_ok = method == "publickey"
                && algo == ED25519
                && crypto::ed25519_verify(&cert.key, &signed.take(), &sig64);
        }
        if !log.auth_ok {
            w.send(&{
                let mut b = Buf::new();
                b.u8(msg::USERAUTH_FAILURE).str("publickey").bool(false);
                b.take()
            })?;
            return Ok(log);
        }
        w.send(&[msg::USERAUTH_SUCCESS])?;

        // One channel.
        let p = w.recv()?;
        if p[0] != msg::CHANNEL_OPEN {
            return Err("the client opened no channel".into());
        }
        let mut c = Cur::new(&p[1..]);
        let kind = c.text()?;
        let their = c.u32()?;
        let _window = c.u32()?;
        let _max = c.u32()?;
        if kind != "session" {
            return Err(format!("the client asked for a {} channel", kind));
        }
        let mut b = Buf::new();
        b.u8(msg::CHANNEL_OPEN_CONFIRMATION)
            .u32(their)
            .u32(0)
            .u32(CHANNEL_WINDOW)
            .u32(CHANNEL_MAX);
        w.send(&b.take())?;

        // Requests, then behave like the thing that was asked for.
        loop {
            let p = w.recv()?;
            match p[0] {
                msg::CHANNEL_REQUEST => {
                    let mut c = Cur::new(&p[1..]);
                    let _ch = c.u32()?;
                    let name = c.text()?;
                    let want_reply = c.bool()?;
                    log.requests.push(name.clone());
                    match name.as_str() {
                        "pty-req" => {
                            log.term = c.text()?;
                            log.cols = c.u32()?;
                        }
                        "exec" => log.command = c.text()?,
                        _ => {}
                    }
                    if want_reply {
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_SUCCESS).u32(their);
                        w.send(&b.take())?;
                    }
                    if name == "exec" {
                        let out = format!("{}\n", log.command.to_uppercase());
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_DATA).u32(their).string(out.as_bytes());
                        w.send(&b.take())?;
                        let code = if log.command.contains("false") { 1u32 } else { 0 };
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_REQUEST)
                            .u32(their)
                            .str("exit-status")
                            .bool(false)
                            .u32(code);
                        w.send(&b.take())?;
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_EOF).u32(their);
                        w.send(&b.take())?;
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_CLOSE).u32(their);
                        w.send(&b.take())?;
                        return Ok(log);
                    }
                    if name == "shell" {
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_DATA).u32(their).string(b"$ ");
                        w.send(&b.take())?;
                    }
                }
                msg::CHANNEL_DATA => {
                    let mut c = Cur::new(&p[1..]);
                    let _ch = c.u32()?;
                    let data = c.string()?;
                    log.typed.extend_from_slice(data);
                    // Echo, the way a shell with a terminal does.
                    let mut b = Buf::new();
                    b.u8(msg::CHANNEL_DATA).u32(their).string(data);
                    w.send(&b.take())?;
                    if log.typed.ends_with(b"exit\n") {
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_REQUEST)
                            .u32(their)
                            .str("exit-status")
                            .bool(false)
                            .u32(0);
                        w.send(&b.take())?;
                        let mut b = Buf::new();
                        b.u8(msg::CHANNEL_CLOSE).u32(their);
                        w.send(&b.take())?;
                        return Ok(log);
                    }
                }
                msg::CHANNEL_EOF | msg::CHANNEL_WINDOW_ADJUST => {}
                msg::CHANNEL_CLOSE => return Ok(log),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, Opts};
    use super::*;

    /// Build an OpenSSH private key file the way `ssh-keygen` writes one, so
    /// the parser is tested against the format rather than against itself.
    fn private_key_file(seed: &[u8; 32]) -> String {
        let pk = crypto::ed25519_public(seed);
        let mut inner = Buf::new();
        inner
            .u32(0x01020304)
            .u32(0x01020304)
            .str(ED25519)
            .string(&pk)
            .string(&{
                let mut sk = seed.to_vec();
                sk.extend_from_slice(&pk);
                sk
            })
            .str("operator@museum");
        let mut body = inner.take();
        // The private section is padded to the cipher's block size with
        // 1, 2, 3... which `none` treats as a block size of eight.
        let mut n = 1u8;
        while body.len() % 8 != 0 {
            body.push(n);
            n += 1;
        }
        let mut outer = Vec::from(&b"openssh-key-v1\0"[..]);
        let mut b = Buf::new();
        b.str("none")
            .str("none")
            .string(&[])
            .u32(1)
            .string(&ed25519_blob(&pk))
            .string(&body);
        outer.extend_from_slice(&b.take());

        let b64 = crypto::b64_encode(&outer);
        let mut out = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
        for chunk in b64.as_bytes().chunks(70) {
            out.push_str(std::str::from_utf8(chunk).unwrap());
            out.push('\n');
        }
        out.push_str("-----END OPENSSH PRIVATE KEY-----\n");
        out
    }

    #[test]
    fn the_wire_types_are_written_the_way_the_rfc_spells_them() {
        let mut b = Buf::new();
        b.string(b"abc");
        assert_eq!(b.take(), vec![0, 0, 0, 3, b'a', b'b', b'c']);

        // mpint is signed, so a value with its top bit set grows a zero byte.
        // The shared secret goes into the exchange hash this way, and getting
        // it wrong fails about half of all handshakes -- which is worse than
        // failing all of them.
        let mut b = Buf::new();
        b.mpint(&[0x80, 0x00]);
        assert_eq!(b.take(), vec![0, 0, 0, 3, 0x00, 0x80, 0x00]);
        let mut b = Buf::new();
        b.mpint(&[0x00, 0x00, 0x7f]);
        assert_eq!(b.take(), vec![0, 0, 0, 1, 0x7f]);
        let mut b = Buf::new();
        b.mpint(&[0, 0, 0]);
        assert_eq!(b.take(), vec![0, 0, 0, 0]);

        let mut b = Buf::new();
        b.list(&["a", "b"]);
        assert_eq!(b.take(), vec![0, 0, 0, 3, b'a', b',', b'b']);
    }

    #[test]
    fn a_truncated_packet_is_a_sentence_rather_than_a_panic() {
        let mut c = Cur::new(&[0, 0, 0, 9, 1, 2]);
        assert!(c.string().is_err());
        let mut c = Cur::new(&[0, 0]);
        assert!(c.u32().is_err());
    }

    #[test]
    fn an_openssh_private_key_reads_back() {
        let (seed, pk) = crypto::ed25519_keypair().unwrap();
        let file = private_key_file(&seed);
        let (got_seed, got_pub) = parse_private_key(&file).unwrap();
        assert_eq!(got_seed, seed);
        assert_eq!(got_pub, pk);

        // A key with a passphrase is refused in words rather than half-read.
        let bad = file.replace("openssh-key-v1", "openssh-key-v1");
        let enc = {
            let mut outer = Vec::from(&b"openssh-key-v1\0"[..]);
            let mut b = Buf::new();
            b.str("aes256-ctr").str("bcrypt").string(&[]).u32(1);
            outer.extend_from_slice(&b.take());
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
                crypto::b64_encode(&outer)
            )
        };
        let e = parse_private_key(&enc).unwrap_err();
        assert!(e.contains("passphrase"), "unhelpful: {}", e);
        assert!(parse_private_key("not a key at all").is_err());
        assert!(parse_private_key(&bad).is_ok());
    }

    #[test]
    fn a_certificate_reads_back_and_every_check_is_made() {
        let (ca_seed, ca_pub) = crypto::ed25519_keypair().unwrap();
        let (_, key) = crypto::ed25519_keypair().unwrap();
        let now = now_secs();
        let blob = fake::make_cert(
            &ca_seed,
            &key,
            CertKind::Host,
            &["museum-01", "museum-01.local"],
            now - 60,
            now + 60,
            "museum-01 host key",
        );
        let cert = Cert::parse(&blob).unwrap();
        assert_eq!(cert.key, key);
        assert_eq!(cert.kind, CertKind::Host);
        assert_eq!(cert.principals, vec!["museum-01", "museum-01.local"]);
        cert.check(&[ca_pub], CertKind::Host, "museum-01", now).unwrap();

        // Every way it can be wrong, and each of them is a refusal.
        let (_, other) = crypto::ed25519_keypair().unwrap();
        assert!(cert.check(&[other], CertKind::Host, "museum-01", now).is_err());
        assert!(cert.check(&[ca_pub], CertKind::User, "museum-01", now).is_err());
        assert!(cert.check(&[ca_pub], CertKind::Host, "museum-02", now).is_err());
        assert!(cert.check(&[ca_pub], CertKind::Host, "museum-01", now + 3600).is_err());
        assert!(cert.check(&[ca_pub], CertKind::Host, "museum-01", now - 3600).is_err());

        // A certificate whose body was edited after signing.
        let mut tampered = blob.clone();
        let at = tampered
            .windows(9)
            .position(|w| w == b"museum-01")
            .unwrap();
        tampered[at + 7] = b'9';
        let t = Cert::parse(&tampered).unwrap();
        assert!(t.check(&[ca_pub], CertKind::Host, "museum-91", now).is_err());
    }

    #[test]
    fn a_certificate_with_no_principals_is_refused() {
        // OpenSSH reads an empty principal list as "anybody". This fleet does
        // not: a certificate that names nothing is a certificate that was
        // issued carelessly.
        let (ca_seed, ca_pub) = crypto::ed25519_keypair().unwrap();
        let (_, key) = crypto::ed25519_keypair().unwrap();
        let now = now_secs();
        let blob = fake::make_cert(&ca_seed, &key, CertKind::Host, &[], now - 60, now + 60, "x");
        let cert = Cert::parse(&blob).unwrap();
        let e = cert
            .check(&[ca_pub], CertKind::Host, "museum-01", now)
            .unwrap_err();
        assert!(e.contains("no principals"), "{}", e);
    }

    #[test]
    fn a_command_runs_on_the_far_end_and_comes_back_with_its_status() {
        let f = fake::serve(Opts::default());
        let (out, code) = Session::run(&f.dial, "uname -a", Duration::from_secs(10)).unwrap();
        assert_eq!(out.trim(), "UNAME -A");
        assert_eq!(code, 0);
        let log = f.handle.join().unwrap().unwrap();
        assert_eq!(log.user, "copal");
        assert!(log.auth_ok, "the server did not accept the signature");
        assert_eq!(log.auth_algo, ED25519_CERT);
        assert_eq!(log.command, "uname -a");
    }

    #[test]
    fn a_command_that_fails_reports_that_it_failed() {
        let f = fake::serve(Opts::default());
        let (_, code) = Session::run(&f.dial, "false", Duration::from_secs(10)).unwrap();
        assert_eq!(code, 1);
        let _ = f.handle.join();
    }

    #[test]
    fn a_terminal_is_asked_for_and_what_is_typed_arrives() {
        // This is Terminal, in miniature: a pty on the far end, a shell, and
        // the operator's keystrokes reaching it.
        let f = fake::serve(Opts::default());
        let mut s = Session::open(&f.dial).unwrap();
        s.pty("xterm-256color", 80, 24).unwrap();
        s.shell().unwrap();

        let start = Instant::now();
        let mut seen = Vec::new();
        while !seen.ends_with(b"$ ") && start.elapsed() < Duration::from_secs(5) {
            seen.extend_from_slice(&s.read().unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(String::from_utf8_lossy(&seen), "$ ");

        s.write(b"exit\n").unwrap();
        let start = Instant::now();
        while s.finished().is_none() && start.elapsed() < Duration::from_secs(5) {
            seen.extend_from_slice(&s.read().unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(String::from_utf8_lossy(&seen).contains("exit"), "no echo");
        assert_eq!(s.finished(), Some(0));

        let log = f.handle.join().unwrap().unwrap();
        assert_eq!(log.term, "xterm-256color");
        assert_eq!(log.cols, 80);
        assert_eq!(log.typed, b"exit\n");
        assert_eq!(log.requests, vec!["pty-req", "shell"]);
    }

    #[test]
    fn the_node_must_prove_its_certificate_four_different_ways() {
        // Each of these is a real attack rather than a configuration mistake,
        // and each is refused at the handshake -- before the operator's
        // certificate has been offered to anybody.
        let cases: &[(&str, Opts, &str)] = &[
            (
                "a CA the fleet has never heard of",
                Opts { foreign_ca: true, ..Default::default() },
                "CA",
            ),
            (
                "a certificate that expired an hour ago",
                Opts { valid_from: -7200, valid_to: -3600, ..Default::default() },
                "not valid now",
            ),
            (
                "a certificate for a different node",
                Opts { principal: "museum-02".into(), ..Default::default() },
                "not for",
            ),
            (
                "a user certificate offered as a host key",
                Opts { kind: CertKind::User, ..Default::default() },
                "check",
            ),
        ];
        for (what, opts, expect) in cases {
            let f = fake::serve(opts.clone());
            let e = match Session::open(&f.dial) {
                Ok(_) => panic!("{} was accepted", what),
                Err(e) => e,
            };
            assert!(e.contains(expect), "{}: unhelpful refusal {:?}", what, e);
            let _ = f.handle.join();
        }
    }

    #[test]
    fn a_node_that_does_not_hold_the_certified_key_is_refused() {
        // The certificate is real, from the real CA, for the right host -- and
        // the exchange hash was signed by something else. This is the check
        // that makes the certificate mean anything at all.
        let f = fake::serve(Opts { wrong_signer: true, ..Default::default() });
        let e = match Session::open(&f.dial) {
            Ok(_) => panic!("a node that did not hold the key was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("did not prove"), "unhelpful: {}", e);
        let _ = f.handle.join();
    }

    #[test]
    fn the_offer_is_narrower_than_the_profile_and_never_wider() {
        // The client may offer only what the profile permits. It may offer
        // less -- see the note at the top about AES -- but a name it can offer
        // that the profile does not list would be a hole in the whitelist.
        let ciphers = offer_ciphers();
        assert!(!ciphers.is_empty(), "the client offers no cipher at all");
        for c in &ciphers {
            assert!(
                profile::P1.ssh_cipher.contains(c),
                "{} is offered and is not on the profile",
                c
            );
        }
        assert!(
            ciphers.iter().all(|c| c.starts_with("chacha20")),
            "AES is being offered, and R4 in docs/wire.md says why it must not be"
        );
        assert_eq!(profile::P1.ssh_host_key, &[ED25519_CERT]);
    }

    #[test]
    fn the_client_takes_the_first_of_its_own_names_the_peer_has() {
        // The client's order decides, which is the protocol's rule -- and the
        // difference between this and the server's order deciding is the whole
        // of R4 in docs/wire.md.
        assert_eq!(pick(&["a", "b"], "c,b,a"), Some("a".into()));
        assert_eq!(pick(&["a", "b"], "b,c"), Some("b".into()));
        assert_eq!(pick(&["a"], "aa,ab"), None, "a prefix was taken for a match");
        assert_eq!(pick(&["a"], ""), None);
    }
}

/// The tests that need a real `sshd`.
///
/// THEY STEP ASIDE WHEN THERE IS NOT ONE, which is the same arrangement
/// `wl.rs` has with a compositor: the suite stays green on a Mac and these
/// prove nothing there, and `tools/ssh-check.sh` is where they mean something.
/// What they add over the fake is everything OpenSSH is strict about and a
/// hand-written server is not -- the exact signature-algorithm name in a
/// certificate request, the principals file, and the option parsing that
/// decides whether the node's sshd is as narrow as `profile.rs` says.
#[cfg(test)]
mod live {
    use super::*;
    use std::path::PathBuf;

    fn dial() -> Option<Dial> {
        let v = |k: &str| std::env::var(k).ok();
        let addr = v("ORRERY_SSH_ADDR")?;
        let d = Dial::from_files(
            &addr,
            &v("ORRERY_SSH_HOST")?,
            &v("ORRERY_SSH_USER")?,
            &PathBuf::from(v("ORRERY_SSH_KEY")?),
            &PathBuf::from(v("ORRERY_SSH_CERT")?),
            &[PathBuf::from(v("ORRERY_SSH_CA")?)],
        )
        .expect("the fixture's own key material should parse");
        Some(d)
    }

    #[test]
    fn a_real_sshd_accepts_the_operator_certificate() {
        let Some(d) = dial() else { return };
        let (out, code) = Session::run(&d, "id -un; uname -s", Duration::from_secs(20)).unwrap();
        assert_eq!(code, 0, "the command failed: {}", out);
        assert!(out.contains(&d.user), "{} did not run as {}", out, d.user);
        assert!(out.contains("Linux"), "{}", out);
    }

    #[test]
    fn a_real_sshd_gives_a_terminal_and_a_shell() {
        let Some(d) = dial() else { return };
        let mut s = Session::open(&d).unwrap();
        s.pty("xterm-256color", 100, 30).unwrap();
        s.shell().unwrap();
        // `tty` prints a device name when there is a terminal and an error
        // when there is not, which is the whole point of asking for one.
        s.write(b"tty; exit\n").unwrap();
        let start = Instant::now();
        let mut seen = Vec::new();
        while s.finished().is_none() && start.elapsed() < Duration::from_secs(20) {
            seen.extend_from_slice(&s.read().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = String::from_utf8_lossy(&seen).to_string();
        assert!(text.contains("/dev/pts/"), "no terminal on the far end: {}", text);
    }

    #[test]
    fn a_real_sshd_is_refused_when_the_ca_is_not_ours() {
        let Some(mut d) = dial() else { return };
        let (_, other) = crypto::ed25519_keypair().unwrap();
        d.cas = vec![other];
        let e = match Session::open(&d) {
            Ok(_) => panic!("a node signed by another CA was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("CA"), "unhelpful: {}", e);
    }

    #[test]
    fn a_real_sshd_is_refused_when_it_is_not_the_node_we_asked_for() {
        let Some(mut d) = dial() else { return };
        d.host = "museum-99".into();
        let e = match Session::open(&d) {
            Ok(_) => panic!("a certificate for another node was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("not for"), "unhelpful: {}", e);
    }

    #[test]
    fn a_real_sshd_refuses_a_certificate_the_ca_did_not_sign() {
        let Some(mut d) = dial() else { return };
        // A key with no certificate behind it: the node should turn it away
        // rather than fall back to anything.
        let (seed, pk) = crypto::ed25519_keypair().unwrap();
        let (ca_seed, _) = crypto::ed25519_keypair().unwrap();
        d.seed = seed;
        d.cert = fake::make_cert(
            &ca_seed,
            &pk,
            CertKind::User,
            &["fleet-operator"],
            now_secs() - 60,
            now_secs() + 600,
            "not the operator",
        );
        let mut c = Conn::open(&d).unwrap();
        let e = match c.authenticate(&d) {
            Ok(()) => panic!("the node accepted a certificate its CA never signed"),
            Err(e) => e,
        };
        assert!(e.contains("refused"), "unhelpful: {}", e);
    }
}
