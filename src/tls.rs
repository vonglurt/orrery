//! TLS 1.3, client, one suite -- the layer RDP has to arrive through.
//!
//! WHY THIS EXISTS. `docs/wire.md` §5: RDP's only acceptable security layer is
//! TLS, because the alternative is RDP's own RC4-and-512-bit-RSA scheme, and
//! `profile.rs` offers `PROTOCOL_SSL` alone. So a console that drives a node's
//! desktop needs a TLS client, and a crate is a fetch that fails on a node.
//!
//! ONE VERSION, ONE GROUP, ONE SUITE, ONE SIGNATURE ALGORITHM. That is not
//! minimalism for its own sake: every branch removed here is a branch an
//! attacker cannot steer into. There is no TLS 1.2 record layer, so there is no
//! MAC-then-encrypt and no padding oracle. There is no RSA key transport, so
//! there is no Bleichenbacher. There is no renegotiation, no compression, no
//! session resumption, no early data, and no downgrade to negotiate -- a server
//! that cannot do TLS 1.3 with X25519 and ChaCha20-Poly1305 is refused with a
//! sentence rather than met halfway.
//!
//! CHACHA IS OFFERED ALONE, AND THAT IS R4. `profile.rs` accepts AES-256-GCM
//! as well, and in SSH the client's preference decides so offering both is
//! free. In TLS THE SERVER CHOOSES, and rustls -- which hypr-rdp is built on --
//! ranks AES above ChaCha. Offering both would hand a Zero 2 the cipher its
//! bitsliced AES runs at half a megabyte a second (`crypto.rs` measures it),
//! which is not a slow session but a dead one. So the offer is one name.
//!
//! MUTUAL TLS IS THE LOCKDOWN. When a client certificate is configured, this
//! answers a `CertificateRequest` with the fleet's own certificate and a
//! signature made with the operator's key. `docs/lockdown.md` §3: the node
//! demanding a client certificate from the fleet CA is THE control that stops
//! mstsc and FreeRDP, because neither can be given one.

use crate::crypto::{self, Digest};
use crate::x509;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Content types.
mod ct {
    pub const CHANGE_CIPHER_SPEC: u8 = 20;
    pub const ALERT: u8 = 21;
    pub const HANDSHAKE: u8 = 22;
    pub const APPLICATION_DATA: u8 = 23;
}

/// Handshake types.
mod hs {
    pub const CLIENT_HELLO: u8 = 1;
    pub const SERVER_HELLO: u8 = 2;
    pub const NEW_SESSION_TICKET: u8 = 4;
    pub const ENCRYPTED_EXTENSIONS: u8 = 8;
    pub const CERTIFICATE: u8 = 11;
    pub const CERTIFICATE_REQUEST: u8 = 13;
    pub const CERTIFICATE_VERIFY: u8 = 15;
    pub const FINISHED: u8 = 20;
    pub const KEY_UPDATE: u8 = 24;
}

/// The one cipher suite: TLS_CHACHA20_POLY1305_SHA256.
const SUITE: u16 = 0x1303;
/// X25519.
const GROUP: u16 = 0x001d;
/// ed25519.
const SIG_ED25519: u16 = 0x0807;
const TLS13: u16 = 0x0304;
const MAX_RECORD: usize = 16384;

/// The server random a HelloRetryRequest carries instead of a random: the
/// SHA-256 of "HelloRetryRequest". Recognised so the refusal can say what
/// happened rather than "that is not a server hello".
const HRR: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// What the console will believe about the far end, and what it will prove
/// about itself.
#[derive(Clone)]
pub struct Config {
    /// The node's name. Sent as SNI and checked against the certificate.
    pub host: String,
    /// The fleet CA's public keys.
    pub cas: Vec<[u8; 32]>,
    /// The certificate chain and private key to present when the node asks --
    /// which, under the lockdown, it always does.
    pub client: Option<(Vec<x509::Cert>, [u8; 32])>,
}

/// A small writer, because TLS counts its lengths in three different widths and
/// every one of them is somewhere a hand-written length goes wrong.
#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) -> &mut W {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut W {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn raw(&mut self, v: &[u8]) -> &mut W {
        self.0.extend_from_slice(v);
        self
    }
    /// A vector whose length is one byte.
    fn v8(&mut self, v: &[u8]) -> &mut W {
        self.u8(v.len() as u8).raw(v)
    }
    /// A vector whose length is two bytes.
    fn v16(&mut self, v: &[u8]) -> &mut W {
        self.u16(v.len() as u16).raw(v)
    }
    /// A vector whose length is three bytes, which is what a certificate list
    /// and a handshake body use.
    fn v24(&mut self, v: &[u8]) -> &mut W {
        let n = v.len();
        self.u8((n >> 16) as u8).u16(n as u16).raw(v)
    }
    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> R<'a> {
        R { b, at: 0 }
    }
    fn need(&self, n: usize) -> Result<(), String> {
        if self.at + n > self.b.len() {
            return Err("a TLS message ended in the middle of a field".into());
        }
        Ok(())
    }
    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.at += 1;
        Ok(self.b[self.at - 1])
    }
    fn u16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        self.at += 2;
        Ok(u16::from_be_bytes([self.b[self.at - 2], self.b[self.at - 1]]))
    }
    fn u24(&mut self) -> Result<usize, String> {
        Ok((self.u8()? as usize) << 16 | self.u16()? as usize)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        self.at += n;
        Ok(&self.b[self.at - n..self.at])
    }
    fn v8(&mut self) -> Result<&'a [u8], String> {
        let n = self.u8()? as usize;
        self.take(n)
    }
    fn v16(&mut self) -> Result<&'a [u8], String> {
        let n = self.u16()? as usize;
        self.take(n)
    }
    fn v24(&mut self) -> Result<&'a [u8], String> {
        let n = self.u24()?;
        self.take(n)
    }
    fn done(&self) -> bool {
        self.at >= self.b.len()
    }
}

/// One direction's keys.
struct Keys {
    key: [u8; 32],
    iv: [u8; 12],
    seq: u64,
    secret: Vec<u8>,
}

impl Keys {
    fn from(secret: &[u8]) -> Result<Keys, String> {
        let key = crypto::hkdf_expand_label::<crypto::Sha256>(secret, "key", b"", 32)?;
        let iv = crypto::hkdf_expand_label::<crypto::Sha256>(secret, "iv", b"", 12)?;
        let mut k = [0u8; 32];
        let mut v = [0u8; 12];
        k.copy_from_slice(&key);
        v.copy_from_slice(&iv);
        Ok(Keys {
            key: k,
            iv: v,
            seq: 0,
            secret: secret.to_vec(),
        })
    }

    /// The record nonce: the sequence number, big-endian in the low eight
    /// bytes, exclusive-ored with the static IV. THE SEQUENCE NUMBER IS NEVER
    /// SENT -- both ends count, and a mismatch shows up as a tag that does not
    /// check rather than as a field somebody can lie about.
    fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        let s = self.seq.to_be_bytes();
        for i in 0..8 {
            n[4 + i] ^= s[i];
        }
        n
    }

    /// A KeyUpdate rolls the traffic secret forward. Nothing else about the
    /// connection changes, and the sequence number restarts.
    fn update(&mut self) -> Result<(), String> {
        let next =
            crypto::hkdf_expand_label::<crypto::Sha256>(&self.secret, "traffic upd", b"", 32)?;
        *self = Keys::from(&next)?;
        Ok(())
    }
}

fn derive_secret(secret: &[u8], label: &str, transcript: &[u8]) -> Result<Vec<u8>, String> {
    crypto::hkdf_expand_label::<crypto::Sha256>(secret, label, &crypto::sha256(transcript), 32)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn alert_word(code: u8) -> &'static str {
    match code {
        0 => "the peer said goodbye",
        40 => "the handshake failed",
        42 => "the node did not like this console's certificate",
        44 => "the node revoked this console's certificate",
        45 => "this console's certificate has expired",
        46 => "the node does not know this console's certificate authority",
        47 => "the node refused this console's certificate",
        48 => "the node does not know this console's certificate authority",
        49 => "the node refused access",
        50 => "the node could not read a message",
        51 => "the node could not verify this console",
        70 => "the node does not speak this version",
        71 => "the node has no cipher in common with this console",
        80 => "the node had an internal error",
        109 => "the node needed a certificate and did not get one",
        112 => "the node does not serve that name",
        116 => "the node required a certificate",
        _ => "the node refused",
    }
}

/// A TLS 1.3 connection.
pub struct Conn {
    s: TcpStream,
    inbuf: Vec<u8>,
    /// Decrypted application data waiting to be read.
    plain: Vec<u8>,
    tx: Option<Keys>,
    rx: Option<Keys>,
    /// The handshake, verbatim, because every secret is derived from a hash of
    /// a prefix of it.
    transcript: Vec<u8>,
    ended: bool,
    /// What the node proved it was. Kept so the console can say which
    /// certificate opened the connection.
    pub peer: Option<x509::Cert>,
}

impl Conn {
    const TIMEOUT: Duration = Duration::from_secs(20);

    /// Connect, handshake, and refuse anything that is not exactly right.
    pub fn connect(addr: &str, cfg: &Config) -> Result<Conn, String> {
        let s = TcpStream::connect(addr).map_err(|e| format!("cannot reach {}: {}", addr, e))?;
        Conn::start(s, cfg)
    }

    /// Take over a socket that has already been talked on.
    ///
    /// RDP NEEDS THIS AND NOTHING ELSE DOES. Its X.224 negotiation happens in
    /// the clear on the same connection, and only then does the stack upgrade
    /// -- so the socket arrives here with bytes already sent on it.
    pub fn start(s: TcpStream, cfg: &Config) -> Result<Conn, String> {
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(Duration::from_millis(20))).ok();
        let mut c = Conn {
            s,
            inbuf: Vec::new(),
            plain: Vec::new(),
            tx: None,
            rx: None,
            transcript: Vec::new(),
            ended: false,
            peer: None,
        };
        c.handshake(cfg)?;
        Ok(c)
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
                Err(ref e)
                    if e.kind() == ErrorKind::ConnectionReset
                        || e.kind() == ErrorKind::ConnectionAborted =>
                {
                    self.ended = true;
                    return Ok(());
                }
                Err(e) => return Err(format!("reading from the node: {}", e)),
            }
        }
    }

    /// One record off the wire, decrypted if there are keys. Returns the inner
    /// content type and the body.
    fn record(&mut self) -> Result<(u8, Vec<u8>), String> {
        let start = Instant::now();
        loop {
            if self.inbuf.len() >= 5 {
                let kind = self.inbuf[0];
                let len = u16::from_be_bytes([self.inbuf[3], self.inbuf[4]]) as usize;
                // The length arrives before anything is authenticated, so it
                // is checked against the standard's own ceiling rather than
                // used to size an allocation.
                if len > MAX_RECORD + 256 {
                    return Err("a TLS record larger than the standard allows".into());
                }
                if self.inbuf.len() >= 5 + len {
                    let header: [u8; 5] = self.inbuf[..5].try_into().unwrap();
                    let body = self.inbuf[5..5 + len].to_vec();
                    self.inbuf.drain(..5 + len);

                    if kind == ct::CHANGE_CIPHER_SPEC {
                        // Sent for the benefit of middleboxes that have not
                        // been told TLS 1.3 exists. It carries nothing and is
                        // not part of the transcript.
                        continue;
                    }
                    let (inner_type, plain) = match self.rx.as_mut() {
                        None => (kind, body),
                        Some(keys) => {
                            let mut buf = body;
                            if buf.len() < 17 {
                                return Err("an encrypted record with no room for a tag".into());
                            }
                            let tag_at = buf.len() - 16;
                            let mut tag = [0u8; 16];
                            tag.copy_from_slice(&buf[tag_at..]);
                            buf.truncate(tag_at);
                            crypto::chacha20_poly1305_open(
                                &keys.key,
                                &keys.nonce(),
                                &header,
                                &mut buf,
                                &tag,
                            )?;
                            keys.seq += 1;
                            // The real content type is the last non-zero byte:
                            // TLS 1.3 hides it under the padding, which is
                            // also why a record can be all padding and must
                            // not loop forever here.
                            let end = match buf.iter().rposition(|b| *b != 0) {
                                Some(i) => i,
                                None => return Err("a record with no content type".into()),
                            };
                            let t = buf[end];
                            buf.truncate(end);
                            (t, buf)
                        }
                    };
                    if inner_type == ct::ALERT {
                        if plain.len() >= 2 {
                            if plain[1] == 0 {
                                self.ended = true;
                                return Err("the node closed the connection".into());
                            }
                            return Err(alert_word(plain[1]).to_string());
                        }
                        return Err("the node sent an alert".into());
                    }
                    return Ok((inner_type, plain));
                }
            }
            if self.ended && self.inbuf.len() < 5 {
                return Err("the node closed the connection".into());
            }
            if start.elapsed() > Self::TIMEOUT {
                return Err("the node stopped answering".into());
            }
            self.pump()?;
        }
    }

    /// One handshake message, which may be smaller or larger than a record.
    fn handshake_message(&mut self, buf: &mut Vec<u8>) -> Result<(u8, Vec<u8>), String> {
        loop {
            if buf.len() >= 4 {
                let len = (buf[1] as usize) << 16 | (buf[2] as usize) << 8 | buf[3] as usize;
                if buf.len() >= 4 + len {
                    let kind = buf[0];
                    let whole = buf[..4 + len].to_vec();
                    buf.drain(..4 + len);
                    // EVERY HANDSHAKE MESSAGE GOES INTO THE TRANSCRIPT, header
                    // included, in the order it arrived -- the secrets are
                    // hashes of exactly these bytes, so a message read but not
                    // recorded produces a Finished that does not match and no
                    // explanation of why.
                    self.transcript.extend_from_slice(&whole);
                    return Ok((kind, whole[4..].to_vec()));
                }
            }
            let (kind, body) = self.record()?;
            if kind != ct::HANDSHAKE {
                return Err(format!("content type {} during the handshake", kind));
            }
            buf.extend_from_slice(&body);
        }
    }

    fn send_record(&mut self, kind: u8, body: &[u8]) -> Result<(), String> {
        for chunk in body.chunks(MAX_RECORD) {
            let mut out = Vec::with_capacity(chunk.len() + 22);
            match self.tx.as_mut() {
                None => {
                    out.push(kind);
                    out.extend_from_slice(&[0x03, 0x01]);
                    out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
                    out.extend_from_slice(chunk);
                }
                Some(keys) => {
                    let mut inner = chunk.to_vec();
                    inner.push(kind);
                    let mut header = vec![ct::APPLICATION_DATA, 0x03, 0x03];
                    header.extend_from_slice(&((inner.len() + 16) as u16).to_be_bytes());
                    let tag = crypto::chacha20_poly1305_seal(
                        &keys.key,
                        &keys.nonce(),
                        &header,
                        &mut inner,
                    );
                    keys.seq += 1;
                    out.extend_from_slice(&header);
                    out.extend_from_slice(&inner);
                    out.extend_from_slice(&tag);
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
        }
        Ok(())
    }

    fn send_handshake(&mut self, kind: u8, body: &[u8]) -> Result<(), String> {
        let mut w = W::default();
        w.u8(kind).v24(body);
        let msg = w.take();
        self.transcript.extend_from_slice(&msg);
        self.send_record(ct::HANDSHAKE, &msg)
    }

    fn client_hello(&mut self, cfg: &Config, public: &[u8; 32]) -> Result<(), String> {
        let mut ext = W::default();
        // server_name. Left out for an address, because SNI is a name and an
        // IP in it is a protocol error several servers answer with an alert.
        if !cfg.host.is_empty() && cfg.host.parse::<std::net::IpAddr>().is_err() {
            let mut sni = W::default();
            let mut entry = W::default();
            entry.u8(0).v16(cfg.host.as_bytes());
            sni.v16(&entry.take());
            ext.u16(0).v16(&sni.take());
        }
        // supported_groups
        let mut g = W::default();
        g.u16(GROUP);
        let mut gs = W::default();
        gs.v16(&g.take());
        ext.u16(10).v16(&gs.take());
        // signature_algorithms
        let mut sa = W::default();
        sa.u16(SIG_ED25519);
        let mut sas = W::default();
        sas.v16(&sa.take());
        ext.u16(13).v16(&sas.take());
        // supported_versions
        let mut sv = W::default();
        sv.u16(TLS13);
        let mut svs = W::default();
        svs.v8(&sv.take());
        ext.u16(43).v16(&svs.take());
        // key_share
        let mut share = W::default();
        share.u16(GROUP).v16(public);
        let mut shares = W::default();
        shares.v16(&share.take());
        ext.u16(51).v16(&shares.take());

        let mut w = W::default();
        w.u16(0x0303).raw(&crypto::random(32)?);
        // A non-empty legacy session id makes the handshake look like a
        // resumption to a middlebox, which is the entire reason TLS 1.3 keeps
        // the field. Empty works with a server and fails in a hotel.
        w.v8(&crypto::random(32)?);
        let mut suites = W::default();
        suites.u16(SUITE);
        w.v16(&suites.take());
        w.v8(&[0]);
        w.v16(&ext.take());
        let body = w.take();
        self.send_handshake(hs::CLIENT_HELLO, &body)
    }

    fn handshake(&mut self, cfg: &Config) -> Result<(), String> {
        let (secret, public) = crypto::x25519_keypair()?;
        self.client_hello(cfg, &public)?;

        // ServerHello, in the clear.
        let mut pending = Vec::new();
        let (kind, body) = self.handshake_message(&mut pending)?;
        if kind != hs::SERVER_HELLO {
            return Err(format!("message {} where a server hello should be", kind));
        }
        let mut r = R::new(&body);
        let _legacy = r.u16()?;
        let random = r.take(32)?;
        if random == HRR {
            // The only group offered is X25519, so a retry can only be asking
            // for one this console does not have.
            return Err("the node wants a key exchange group this console does not offer".into());
        }
        let _session = r.v8()?;
        let suite = r.u16()?;
        if suite != SUITE {
            return Err(
                "the node chose a cipher this console does not speak -- it must be \
                 chacha20-poly1305"
                    .into(),
            );
        }
        let _comp = r.u8()?;
        let exts = r.v16()?;
        let mut peer_share: Option<[u8; 32]> = None;
        let mut version = 0u16;
        let mut e = R::new(exts);
        while !e.done() {
            let which = e.u16()?;
            let data = e.v16()?;
            match which {
                43 => version = R::new(data).u16()?,
                51 => {
                    let mut k = R::new(data);
                    let group = k.u16()?;
                    let key = k.v16()?;
                    if group != GROUP || key.len() != 32 {
                        return Err("the node answered with another group".into());
                    }
                    let mut p = [0u8; 32];
                    p.copy_from_slice(key);
                    peer_share = Some(p);
                }
                _ => {}
            }
        }
        if version != TLS13 {
            return Err("the node does not speak TLS 1.3, and there is no older one here".into());
        }
        let peer = peer_share.ok_or("the node sent no key share")?;
        let shared = crypto::x25519(&secret, &peer)?;

        // The key schedule, RFC 8446 §7.1.
        let zeros = [0u8; 32];
        let early = crypto::hkdf_extract::<crypto::Sha256>(&[], &zeros);
        let derived = derive_secret(&early, "derived", b"")?;
        let handshake = crypto::hkdf_extract::<crypto::Sha256>(&derived, &shared);
        let c_hs = derive_secret(&handshake, "c hs traffic", &self.transcript)?;
        let s_hs = derive_secret(&handshake, "s hs traffic", &self.transcript)?;
        self.rx = Some(Keys::from(&s_hs)?);
        let client_keys = Keys::from(&c_hs)?;

        // Everything from here is encrypted.
        let (kind, _body) = self.handshake_message(&mut pending)?;
        if kind != hs::ENCRYPTED_EXTENSIONS {
            return Err(format!("message {} where encrypted extensions should be", kind));
        }

        let mut asked_for_certificate = false;
        let mut cert_request_context = Vec::new();
        let mut chain: Vec<x509::Cert> = Vec::new();
        // WHERE THE SERVER'S HALF ENDS. The application secrets are hashes of
        // the transcript up to and including the server's Finished and nothing
        // after it, so the mark is taken there rather than worked out later by
        // subtracting what the client went on to send. Deriving it by
        // arithmetic at the end happened to be right when the client sent only
        // a Finished, and silently wrong the moment it also sent a certificate
        // -- a handshake that completes and then cannot decrypt a byte.
        let after_server_finished;

        loop {
            let (kind, body) = self.handshake_message(&mut pending)?;
            match kind {
                hs::CERTIFICATE_REQUEST => {
                    let mut r = R::new(&body);
                    cert_request_context = r.v8()?.to_vec();
                    asked_for_certificate = true;
                }
                hs::CERTIFICATE => {
                    let mut r = R::new(&body);
                    let _context = r.v8()?;
                    let list = r.v24()?;
                    let mut l = R::new(list);
                    while !l.done() {
                        let der = l.v24()?;
                        let _exts = l.v16()?;
                        chain.push(x509::Cert::parse(der)?);
                    }
                    // THE CHAIN IS CHECKED HERE, BEFORE ITS SIGNATURE OVER THE
                    // TRANSCRIPT IS LOOKED AT. Both checks are needed and the
                    // order matters: this one says the key belongs to the
                    // node, the next says the far end holds it.
                    x509::verify(&chain, &cfg.cas, &cfg.host, now_secs())?;
                    self.peer = chain.first().cloned();
                }
                hs::CERTIFICATE_VERIFY => {
                    // The hash covers everything up to but not including this
                    // message, which `handshake_message` has already appended
                    // -- so the prefix is taken before it was added.
                    let prefix = &self.transcript[..self.transcript.len() - 4 - body.len()];
                    let hash = crypto::sha256(prefix);
                    let mut r = R::new(&body);
                    let algo = r.u16()?;
                    let sig = r.v16()?;
                    if algo != SIG_ED25519 {
                        return Err("the node signed with something other than ed25519".into());
                    }
                    if sig.len() != 64 {
                        return Err("the node's signature is the wrong size".into());
                    }
                    let cert = self
                        .peer
                        .as_ref()
                        .ok_or("the node signed before it sent a certificate")?;
                    let mut signed = vec![0x20u8; 64];
                    signed.extend_from_slice(b"TLS 1.3, server CertificateVerify");
                    signed.push(0);
                    signed.extend_from_slice(&hash);
                    let mut s64 = [0u8; 64];
                    s64.copy_from_slice(sig);
                    if !crypto::ed25519_verify(&cert.key, &signed, &s64) {
                        return Err("the node did not prove it holds its certificate's key".into());
                    }
                }
                hs::FINISHED => {
                    let prefix = &self.transcript[..self.transcript.len() - 4 - body.len()];
                    let key = crypto::hkdf_expand_label::<crypto::Sha256>(
                        &s_hs, "finished", b"", 32,
                    )?;
                    let want = crypto::hmac::<crypto::Sha256>(&key, &crypto::sha256(prefix));
                    if !crypto::ct_eq(&want, &body) {
                        return Err("the node's finished message does not check out".into());
                    }
                    after_server_finished = self.transcript.len();
                    break;
                }
                n => return Err(format!("message {} during the handshake", n)),
            }
        }
        if self.peer.is_none() {
            return Err("the node never sent a certificate".into());
        }

        // The client's half. The transcript for the client's own signature and
        // Finished includes everything sent so far, including its certificate.
        self.tx = Some(client_keys);
        if asked_for_certificate {
            match &cfg.client {
                Some((chain, seed)) => {
                    let mut list = W::default();
                    for c in chain {
                        list.v24(&c.der).u16(0);
                    }
                    let mut w = W::default();
                    w.v8(&cert_request_context).v24(&list.take());
                    let body = w.take();
                    self.send_handshake(hs::CERTIFICATE, &body)?;

                    let hash = crypto::sha256(&self.transcript);
                    let mut signed = vec![0x20u8; 64];
                    signed.extend_from_slice(b"TLS 1.3, client CertificateVerify");
                    signed.push(0);
                    signed.extend_from_slice(&hash);
                    let sig = crypto::ed25519_sign(seed, &signed);
                    let mut w = W::default();
                    w.u16(SIG_ED25519).v16(&sig);
                    let body = w.take();
                    self.send_handshake(hs::CERTIFICATE_VERIFY, &body)?;
                }
                None => {
                    // An empty certificate message is the protocol's way of
                    // saying "I have none". The node will refuse, and the
                    // alert that comes back is a better explanation than
                    // anything this console could invent.
                    let mut w = W::default();
                    w.v8(&cert_request_context).v24(&[]);
                    let body = w.take();
                    self.send_handshake(hs::CERTIFICATE, &body)?;
                }
            }
        }

        let key = crypto::hkdf_expand_label::<crypto::Sha256>(&c_hs, "finished", b"", 32)?;
        let verify = crypto::hmac::<crypto::Sha256>(&key, &crypto::sha256(&self.transcript));
        self.send_handshake(hs::FINISHED, &verify)?;

        // Application keys. The server's are derived from the transcript up to
        // its Finished; the client's from the same point, which is why this
        // happens after the server's Finished was verified and before the
        // client's was added.
        let derived = derive_secret(&handshake, "derived", b"")?;
        let master = crypto::hkdf_extract::<crypto::Sha256>(&derived, &zeros);
        let upto = &self.transcript[..after_server_finished];
        let c_ap = derive_secret(&master, "c ap traffic", upto)?;
        let s_ap = derive_secret(&master, "s ap traffic", upto)?;
        self.tx = Some(Keys::from(&c_ap)?);
        self.rx = Some(Keys::from(&s_ap)?);
        Ok(())
    }

    /// Whatever has arrived and been decrypted. Never blocks.
    pub fn read(&mut self) -> Result<Vec<u8>, String> {
        self.pump()?;
        while self.inbuf.len() >= 5 {
            let len = u16::from_be_bytes([self.inbuf[3], self.inbuf[4]]) as usize;
            if self.inbuf.len() < 5 + len {
                break;
            }
            let (kind, body) = self.record()?;
            match kind {
                ct::APPLICATION_DATA => self.plain.extend_from_slice(&body),
                ct::HANDSHAKE => self.post_handshake(&body)?,
                _ => {}
            }
        }
        Ok(std::mem::take(&mut self.plain))
    }

    /// Tickets and key updates, which arrive whenever the server feels like it.
    fn post_handshake(&mut self, body: &[u8]) -> Result<(), String> {
        let mut r = R::new(body);
        while !r.done() {
            let kind = r.u8()?;
            let msg = r.v24()?;
            match kind {
                // A resumption ticket for a session this console will never
                // resume: read and dropped rather than an error, because
                // refusing one would end a perfectly good connection.
                hs::NEW_SESSION_TICKET => {}
                hs::KEY_UPDATE => {
                    if let Some(k) = self.rx.as_mut() {
                        k.update()?;
                    }
                    // request_update == 1 means the peer wants ours rolled too,
                    // and not answering is a connection that dies later for no
                    // visible reason.
                    if msg.first().copied().unwrap_or(0) == 1 {
                        let mut w = W::default();
                        w.u8(hs::KEY_UPDATE).v24(&[0]);
                        let out = w.take();
                        self.send_record(ct::HANDSHAKE, &out)?;
                        if let Some(k) = self.tx.as_mut() {
                            k.update()?;
                        }
                    }
                }
                n => return Err(format!("message {} after the handshake", n)),
            }
        }
        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.send_record(ct::APPLICATION_DATA, data)
    }

    pub fn ended(&self) -> bool {
        self.ended && self.inbuf.len() < 5
    }

    /// Say goodbye properly. A connection that just drops is indistinguishable
    /// from one that was cut, which matters to the far end's logs.
    pub fn close(&mut self) {
        let _ = self.send_record(ct::ALERT, &[1, 0]);
        let _ = self.s.shutdown(std::net::Shutdown::Both);
        self.ended = true;
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if !self.ended {
            self.close();
        }
    }
}

/// The tests that need a real TLS server.
///
/// THERE IS NO FAKE SERVER HERE AND THERE MUST NOT BE. A hand-written one
/// would agree with this file's reading of RFC 8446, including wherever that
/// reading is wrong, and the key schedule is exactly the sort of thing a reader
/// gets subtly wrong and then implements twice. `tools/tls-check.sh` runs
/// `openssl s_server` -- which is not this program's opinion of TLS -- and
/// these step aside when it is not there.
#[cfg(test)]
mod tests {
    use super::*;

    fn env(k: &str) -> Option<String> {
        std::env::var(k).ok()
    }

    fn ca() -> Vec<[u8; 32]> {
        let pem = std::fs::read_to_string(env("ORRERY_TLS_CA").unwrap()).unwrap();
        x509::from_pem(&pem)
            .unwrap()
            .iter()
            .map(|c| c.key)
            .collect()
    }

    fn client() -> Option<(Vec<x509::Cert>, [u8; 32])> {
        let cert = std::fs::read_to_string(env("ORRERY_TLS_CLIENT_CERT")?).ok()?;
        let key = std::fs::read_to_string(env("ORRERY_TLS_CLIENT_KEY")?).ok()?;
        Some((
            x509::from_pem(&cert).unwrap(),
            x509::key_from_pem(&key).unwrap(),
        ))
    }

    fn cfg(host: &str) -> Config {
        Config {
            host: host.to_string(),
            cas: ca(),
            client: client(),
        }
    }

    fn talk(c: &mut Conn, line: &str) -> String {
        c.write(line.as_bytes()).unwrap();
        let start = Instant::now();
        let mut seen = Vec::new();
        while start.elapsed() < Duration::from_secs(10) {
            seen.extend_from_slice(&c.read().unwrap());
            if seen.contains(&b'\n') {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        String::from_utf8_lossy(&seen).to_string()
    }

    #[test]
    fn a_real_server_proves_who_it_is_and_then_talks() {
        let Some(addr) = env("ORRERY_TLS_PLAIN") else { return };
        let mut c = Conn::connect(&addr, &cfg("museum-01")).unwrap();
        // The certificate is the node's, and the console can say so.
        let peer = c.peer.clone().expect("no certificate kept");
        assert!(peer.matches("museum-01"), "{:?}", peer.names);
        // `s_server -rev` sends each line back reversed, which is the smallest
        // proof that the record layer works in both directions.
        assert_eq!(talk(&mut c, "museum\n").trim(), "muesum");
        c.close();
    }

    #[test]
    fn the_node_demanding_a_client_certificate_is_the_lockdown_working() {
        let Some(addr) = env("ORRERY_TLS_MUTUAL") else { return };
        // The fixture has to actually have a certificate, or the second half
        // of this test passes for the wrong reason.
        assert!(
            client().is_some(),
            "no client certificate in the fixture: {:?} {:?}",
            env("ORRERY_TLS_CLIENT_CERT"),
            env("ORRERY_TLS_CLIENT_KEY")
        );
        // With the fleet's certificate: in.
        let mut c = Conn::connect(&addr, &cfg("museum-01")).unwrap();
        assert_eq!(talk(&mut c, "orrery\n").trim(), "yrerro");
        c.close();

        // Without it: out, at the handshake, before anything else happens.
        // THIS IS WHAT STOPS MSTSC -- a standard client has no fleet
        // certificate and cannot be given one.
        let mut without = cfg("museum-01");
        without.client = None;
        let e = match Conn::connect(&addr, &without) {
            Ok(mut c) => {
                // OPENSSL FINISHES THE HANDSHAKE AND THEN REFUSES, which is
                // allowed: the client's Finished is the last message the
                // server waits for, and the alert comes back with what would
                // have been the first record. So the refusal shows up on the
                // first read rather than at `connect`, and a test that
                // unwrapped that read would fail on the correct behaviour.
                c.write(b"orrery\n").unwrap();
                let start = Instant::now();
                let mut said = Vec::new();
                let refused = loop {
                    match c.read() {
                        Ok(b) => said.extend_from_slice(&b),
                        Err(e) => break e,
                    }
                    if start.elapsed() > Duration::from_secs(5) {
                        break String::new();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert!(
                    !String::from_utf8_lossy(&said).contains("yrerro"),
                    "a client with no certificate got in"
                );
                assert!(!refused.is_empty(), "the node neither answered nor refused");
                return;
            }
            Err(e) => e,
        };
        assert!(!e.is_empty(), "an empty refusal");
    }

    #[test]
    fn a_certificate_from_another_authority_is_refused() {
        let Some(addr) = env("ORRERY_TLS_PLAIN") else { return };
        let mut wrong = cfg("museum-01");
        let (_, other) = crypto::ed25519_keypair().unwrap();
        wrong.cas = vec![other];
        let e = match Conn::connect(&addr, &wrong) {
            Ok(_) => panic!("a node signed by another CA was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("does not know"), "unhelpful: {}", e);
    }

    #[test]
    fn a_certificate_for_another_node_is_refused() {
        let Some(addr) = env("ORRERY_TLS_PLAIN") else { return };
        let e = match Conn::connect(&addr, &cfg("museum-99")) {
            Ok(_) => panic!("a certificate for another node was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("is for"), "unhelpful: {}", e);
    }

    #[test]
    fn a_server_that_will_only_do_aes_is_refused_rather_than_crawled_to() {
        // R4, as a test. The console offers ChaCha alone, so a server that
        // has only AES has nothing in common with it -- which is the intended
        // outcome, because AES here runs at half a megabyte a second.
        let Some(addr) = env("ORRERY_TLS_AES") else { return };
        let e = match Conn::connect(&addr, &cfg("museum-01")) {
            Ok(_) => panic!("the AES-only server was accepted"),
            Err(e) => e,
        };
        assert!(!e.is_empty(), "an empty refusal");
    }

    #[test]
    fn the_offer_is_one_suite_one_group_one_signature() {
        // The ClientHello is built from constants; this is the test that the
        // constants are the ones profile.rs names, and that nothing has
        // quietly grown a second option.
        let p = crate::profile::P1;
        assert_eq!(p.tls_versions, &[TLS13]);
        assert_eq!(p.tls_groups, &["x25519"]);
        assert_eq!(p.tls_sig_algs, &["ed25519"]);
        assert!(
            p.tls_suites.contains(&"TLS_CHACHA20_POLY1305_SHA256"),
            "the profile does not name the one suite this offers"
        );
        // The profile accepts AES as well, and this client deliberately does
        // not ASK for it. See R4 in docs/wire.md.
        assert_eq!(SUITE, 0x1303, "the offered suite is not chacha20-poly1305");
    }

    #[test]
    fn the_record_nonce_is_the_sequence_number_xored_into_the_iv() {
        // RFC 8446 §5.3, and the one piece of the record layer with no test
        // vector of its own -- a wrong nonce is a tag that never checks and no
        // clue why.
        let mut k = Keys::from(&[7u8; 32]).unwrap();
        let first = k.nonce();
        k.seq = 1;
        let second = k.nonce();
        assert_ne!(first, second);
        assert_eq!(first[..4], second[..4], "the first four bytes are the IV");
        let mut want = k.iv;
        want[11] ^= 1;
        assert_eq!(second, want);

        k.seq = 0x0102_0304_0506_0708;
        let n = k.nonce();
        for i in 0..8 {
            assert_eq!(n[4 + i], k.iv[4 + i] ^ (i as u8 + 1));
        }
    }

    #[test]
    fn a_key_update_moves_the_secret_and_restarts_the_count() {
        let mut k = Keys::from(&[9u8; 32]).unwrap();
        let before = (k.key, k.iv);
        k.seq = 42;
        k.update().unwrap();
        assert_ne!((k.key, k.iv), before, "the key did not change");
        assert_eq!(k.seq, 0, "the sequence number did not restart");
    }
}
