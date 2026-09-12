//! RDP, far enough to see a node's desktop and drive it.
//!
//! WHAT THIS TALKS TO. `docs/wire.md` §6 assumed hypr-rdp, and R2 says plainly
//! that nothing here has ever spoken to one. So this is written against a
//! server that does exist and can be run: FreeRDP's own sample server, in a
//! container, via `tools/rdp-check.sh`. That is a different implementation by
//! different people, which is the property that matters -- a fake server would
//! agree with this file's reading of MS-RDPBCGR including wherever that reading
//! is wrong. When a node with hypr-rdp on it exists, this is the client that
//! goes and finds out what it negotiates.
//!
//! THE STACK, WHICH IS FOUR PROTOCOLS DEEP BEFORE A PIXEL MOVES:
//!
//!   TPKT / X.224     a length and a connection request, from 1984
//!   TLS 1.3          `tls.rs`, with the fleet's mutual authentication
//!   MCS (T.125)      a conference with one participant, BER and PER encoded
//!   RDP              capability exchange, then updates and input
//!
//! Every layer is there because the one above it refuses to start without it,
//! and none of them can be skipped.
//!
//! PROTOCOL_SSL AND NOTHING ELSE. The negotiation request asks for TLS alone:
//! not `PROTOCOL_RDP`, which is RC4 and a 512-bit RSA key and is broken; not
//! `PROTOCOL_HYBRID`, which is CredSSP and therefore NTLM, a design whose whole
//! purpose is to carry a credential to a machine before that machine has proved
//! anything. `profile.rs` says the same thing in one number and this is the
//! code that sends it.
//!
//! SIXTEEN BITS PER PIXEL, AND NO DRAWING ORDERS. The order capability set
//! advertises support for nothing, which tells the server to send bitmaps
//! rather than drawing commands -- the same choice `rfb.rs` made in asking for
//! Raw and CopyRect, and for the same reason: a drawing-order interpreter is a
//! second rendering engine to keep correct. 16bpp halves the bandwidth of the
//! arithmetic in wire.md §6 and is what the interleaved RLE decoder below
//! understands.

use crate::tls;
use crate::x509;
use std::time::{Duration, Instant};

/// What a session needs to reach a node's desktop.
#[derive(Clone)]
pub struct Dial {
    pub addr: String,
    /// The node's name, checked against its certificate.
    pub host: String,
    pub user: String,
    pub domain: String,
    pub cas: Vec<[u8; 32]>,
    pub client: Option<(Vec<x509::Cert>, [u8; 32])>,
    pub width: u16,
    pub height: u16,
}

/// The negotiation flags, in the one combination this console will accept.
const PROTOCOL_SSL: u32 = 1;

// ---------------------------------------------------------------------------
// TPKT and X.224
// ---------------------------------------------------------------------------

/// A little writer. RDP counts lengths big-endian at one layer and
/// little-endian at the next, which is the single most reliable source of bugs
/// in the whole protocol, so each has its own named method.
#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) -> &mut W {
        self.0.push(v);
        self
    }
    pub fn be16(&mut self, v: u16) -> &mut W {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn le16(&mut self, v: u16) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn be32(&mut self, v: u32) -> &mut W {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn le32(&mut self, v: u32) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn raw(&mut self, v: &[u8]) -> &mut W {
        self.0.extend_from_slice(v);
        self
    }
    pub fn zeros(&mut self, n: usize) -> &mut W {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    /// A fixed-width UTF-16LE field, null-terminated and padded.
    pub fn utf16_fixed(&mut self, s: &str, bytes: usize) -> &mut W {
        let start = self.0.len();
        for u in s.encode_utf16() {
            if self.0.len() - start + 2 > bytes.saturating_sub(2) {
                break;
            }
            self.0.extend_from_slice(&u.to_le_bytes());
        }
        self.0.resize(start + bytes, 0);
        self
    }
    /// A counted UTF-16LE string with its terminator, as the Info PDU wants.
    pub fn utf16(&mut self, s: &str) -> &mut W {
        for u in s.encode_utf16() {
            self.0.extend_from_slice(&u.to_le_bytes());
        }
        self.0.extend_from_slice(&[0, 0]);
        self
    }
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

pub struct R<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> R<'a> {
        R { b, at: 0 }
    }
    fn need(&self, n: usize) -> Result<(), String> {
        if self.at + n > self.b.len() {
            return Err("an RDP message ended in the middle of a field".into());
        }
        Ok(())
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.at += 1;
        Ok(self.b[self.at - 1])
    }
    pub fn be16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        self.at += 2;
        Ok(u16::from_be_bytes([self.b[self.at - 2], self.b[self.at - 1]]))
    }
    pub fn le16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        self.at += 2;
        Ok(u16::from_le_bytes([self.b[self.at - 2], self.b[self.at - 1]]))
    }
    pub fn le32(&mut self) -> Result<u32, String> {
        Ok(self.le16()? as u32 | (self.le16()? as u32) << 16)
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        self.at += n;
        Ok(&self.b[self.at - n..self.at])
    }
    pub fn skip(&mut self, n: usize) -> Result<(), String> {
        self.take(n).map(|_| ())
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.at..]
    }
    pub fn done(&self) -> bool {
        self.at >= self.b.len()
    }
}

/// Wrap a payload in a TPKT header: version 3, and a big-endian total length.
fn tpkt(payload: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(3).u8(0).be16((payload.len() + 4) as u16).raw(payload);
    w.take()
}

/// The X.224 connection request, with the negotiation block RDP bolted on.
fn connection_request(user: &str) -> Vec<u8> {
    // The cookie is how a load balancer picks a back end. A fleet node is not
    // behind one, and it is sent anyway because some servers log its absence
    // and one or two older ones refuse without it.
    let cookie = format!("Cookie: mstshash={}\r\n", user);
    let mut body = W::default();
    body.u8(0xe0) // CR
        .be16(0) // destination reference
        .be16(0) // source reference
        .u8(0) // class 0
        .raw(cookie.as_bytes())
        .u8(0x01) // TYPE_RDP_NEG_REQ
        .u8(0x00) // flags
        .le16(8) // length, and it is little-endian here
        .le32(PROTOCOL_SSL);
    let body = body.take();
    let mut w = W::default();
    w.u8(body.len() as u8).raw(&body);
    tpkt(&w.take())
}

/// Read the connection confirm and insist the server chose TLS.
fn connection_confirm(b: &[u8]) -> Result<(), String> {
    let mut r = R::new(b);
    if r.u8()? != 3 {
        return Err("that is not a TPKT header -- is anything listening?".into());
    }
    r.skip(1)?;
    let _len = r.be16()?;
    let _li = r.u8()?;
    let code = r.u8()?;
    if code != 0xd0 {
        return Err(format!("the node answered X.224 code {:#x}", code));
    }
    r.skip(5)?;
    if r.done() {
        // No negotiation response at all means a server that only speaks the
        // original RDP security, which is RC4 and a 512-bit RSA key.
        return Err("the node offers no TLS -- and this console speaks nothing else".into());
    }
    let kind = r.u8()?;
    let _flags = r.u8()?;
    let _len = r.le16()?;
    let value = r.le32()?;
    match kind {
        0x02 if value == PROTOCOL_SSL => Ok(()),
        0x02 => Err(format!(
            "the node chose protocol {}, and this console asked for TLS alone",
            value
        )),
        0x03 => Err(format!(
            "the node refused the connection: {}",
            match value {
                1 => "it requires NLA, which means CredSSP and NTLM, and this console refuses it",
                2 => "TLS is not configured on it",
                3 => "it requires a smart card",
                5 => "it requires credentials this console does not have",
                6 => "it refused the protocol entirely",
                _ => "no reason given",
            }
        )),
        n => Err(format!("an X.224 negotiation message of type {}", n)),
    }
}

// ---------------------------------------------------------------------------
// MCS -- a conference call with one participant
// ---------------------------------------------------------------------------
//
// T.125 is a 1990s multipoint conferencing protocol, encoded in BER, carrying
// a T.124 conference-create exchange encoded in PER, inside which RDP puts its
// own little-endian structures. NOTHING IN HERE IS RDP'S IDEA; it is a stack
// Microsoft adopted wholesale, and the only sane way to write it is to emit
// exactly what every other client emits and to say so.

fn ber(tag: &[u8], body: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.raw(tag);
    if body.len() < 0x80 {
        w.u8(body.len() as u8);
    } else {
        w.u8(0x82).be16(body.len() as u16);
    }
    w.raw(body);
    w.take()
}

fn ber_int(v: u32) -> Vec<u8> {
    let body: Vec<u8> = if v < 0x80 {
        vec![v as u8]
    } else if v < 0x8000 {
        vec![(v >> 8) as u8, v as u8]
    } else {
        vec![0, (v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8]
    };
    // The leading zero above keeps a large value positive: BER integers are
    // signed and `maxChannelIds` of 0xFFFF read as -1 is a conference with no
    // channels in it.
    ber(&[0x02], &body)
}

fn domain_params(vals: [u32; 8]) -> Vec<u8> {
    let mut body = Vec::new();
    for v in vals {
        body.extend_from_slice(&ber_int(v));
    }
    ber(&[0x30], &body)
}

/// The PER length prefix: one byte under 128, two with the top bit set above.
fn per_len(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else {
        vec![0x80 | (n >> 8) as u8, n as u8]
    }
}

/// The client's three data blocks, which is what the whole MCS ceremony exists
/// to deliver.
fn client_data(d: &Dial) -> Vec<u8> {
    let mut core = W::default();
    core.le16(0xc001) // CS_CORE
        .le16(216)
        .le32(0x0008_0004) // RDP 5 and later
        .le16(d.width)
        .le16(d.height)
        .le16(0xca01) // legacy colour depth: 8bpp, superseded below
        .le16(0xaa03) // SAS sequence
        .le32(0x0409) // keyboard layout: US, and keymap.rs agrees
        .le32(2600) // client build
        .utf16_fixed("orrery", 32)
        .le32(4) // keyboard type: IBM enhanced
        .le32(0)
        .le32(12) // function keys
        .zeros(64) // ime file name
        .le16(0xca01) // post-beta colour depth
        .le16(1) // product id
        .le32(0) // serial number
        .le16(0x0010) // HIGH_COLOR_16BPP -- see the note at the top
        .le16(0x0001 | 0x0002 | 0x0004) // 24, 16 and 15bpp are all acceptable
        .le16(0x0001) // RNS_UD_CS_SUPPORT_ERRINFO_PDU
        .zeros(64) // client product id
        .u8(0) // connection type
        .u8(0)
        // THE SERVER'S CHOICE, ECHOED BACK. A client that sends something
        // other than the protocol the server selected is told to go away, and
        // this is a field people forget because nothing reads it locally.
        .le32(PROTOCOL_SSL);

    let mut sec = W::default();
    sec.le16(0xc002) // CS_SECURITY
        .le16(12)
        // No RDP encryption methods at all: TLS is underneath, and asking for
        // RC4 as well would be asking for the thing profile.rs forbids.
        .le32(0)
        .le32(0);

    let mut net = W::default();
    net.le16(0xc003) // CS_NET
        .le16(8)
        .le32(0); // no virtual channels: no clipboard, no drives, no sound

    let mut out = core.take();
    out.extend_from_slice(&sec.take());
    out.extend_from_slice(&net.take());
    out
}

/// Connect Initial, with the conference-create request inside it.
fn connect_initial(d: &Dial) -> Vec<u8> {
    let blocks = client_data(d);

    // The PER prologue of a ConferenceCreateRequest. These twelve bytes are
    // the same in every client ever written: choice, selection, the conference
    // name "1", one user-data set, and the H.221 key "Duca" that says the
    // payload is a client's.
    const GCC_PROLOGUE: [u8; 12] = [
        0x00, 0x08, 0x00, 0x10, 0x00, 0x01, 0xc0, 0x00, b'D', b'u', b'c', b'a',
    ];
    let mut gcc = W::default();
    gcc.raw(&[0x00, 0x05, 0x00, 0x14, 0x7c, 0x00, 0x01]) // t124 identifier
        .raw(&per_len(blocks.len() + 14))
        .raw(&GCC_PROLOGUE)
        .raw(&per_len(blocks.len()))
        .raw(&blocks);
    let user_data = gcc.take();

    let mut body = Vec::new();
    body.extend_from_slice(&ber(&[0x04], &[1])); // callingDomainSelector
    body.extend_from_slice(&ber(&[0x04], &[1])); // calledDomainSelector
    body.extend_from_slice(&ber(&[0x01], &[0xff])); // upwardFlag
    body.extend_from_slice(&domain_params([34, 2, 0, 1, 0, 1, 0xffff, 2]));
    body.extend_from_slice(&domain_params([1, 1, 1, 1, 0, 1, 0x420, 2]));
    body.extend_from_slice(&domain_params([0xffff, 0xfc17, 0xffff, 1, 0, 1, 0xffff, 2]));
    body.extend_from_slice(&ber(&[0x04], &user_data));

    let initial = ber(&[0x7f, 0x65], &body);
    tpkt(&x224_data(&initial))
}

/// The X.224 data header that every MCS message rides on: three bytes saying
/// "this is the whole message".
fn x224_data(payload: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2).u8(0xf0).u8(0x80).raw(payload);
    w.take()
}

/// Find the server's data blocks in a Connect Response.
///
/// BY SEARCHING FOR "McDn" RATHER THAN BY WALKING THE ENCODING. The response
/// is BER wrapping PER wrapping a ConferenceCreateResponse whose optional
/// fields vary between servers, and a walk that gets one optional field wrong
/// reads the rest as rubbish. The H.221 key is four bytes that appear exactly
/// once and mark where the server's own structures begin. Everything after it
/// is length-checked properly.
fn server_blocks(b: &[u8]) -> Result<Vec<(u16, Vec<u8>)>, String> {
    let at = b
        .windows(4)
        .position(|w| w == b"McDn")
        .ok_or("the node's answer carries no server data")?;
    let mut r = R::new(&b[at + 4..]);
    // A PER length, one or two bytes.
    let first = r.u8()?;
    if first & 0x80 != 0 {
        let _low = r.u8()?;
    }
    let mut out = Vec::new();
    while !r.done() {
        if r.rest().len() < 4 {
            break;
        }
        let kind = r.le16()?;
        let len = r.le16()? as usize;
        if len < 4 || len - 4 > r.rest().len() {
            return Err("a server data block with a length nobody could mean".into());
        }
        out.push((kind, r.take(len - 4)?.to_vec()));
    }
    Ok(out)
}

mod mcs {
    pub const ERECT_DOMAIN: u8 = 0x04;
    pub const ATTACH_USER_REQUEST: u8 = 0x28;
    pub const ATTACH_USER_CONFIRM: u8 = 0x2e;
    pub const CHANNEL_JOIN_REQUEST: u8 = 0x38;
    pub const CHANNEL_JOIN_CONFIRM: u8 = 0x3e;
    pub const SEND_DATA_REQUEST: u8 = 0x64;
    pub const SEND_DATA_INDICATION: u8 = 0x68;
    pub const DISCONNECT_PROVIDER: u8 = 0x21;
}

// ---------------------------------------------------------------------------
// The capability exchange
// ---------------------------------------------------------------------------

mod pdu {
    /// Share control PDU types, already carrying the protocol version bits.
    pub const DEMAND_ACTIVE: u16 = 0x11;
    pub const CONFIRM_ACTIVE: u16 = 0x13;
    pub const DEACTIVATE_ALL: u16 = 0x16;
    pub const DATA: u16 = 0x17;

    /// Share data PDU types.
    pub const UPDATE: u8 = 2;
    pub const CONTROL: u8 = 20;
    pub const INPUT: u8 = 28;
    pub const SYNCHRONIZE: u8 = 31;
    pub const FONT_LIST: u8 = 39;
    pub const FONT_MAP: u8 = 40;
    pub const ERROR_INFO: u8 = 47;
}

/// One capability set, written the way the server expects to read it.
fn cap(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.le16(kind).le16((body.len() + 4) as u16).raw(body);
    w.take()
}

/// What this console can do, which is deliberately almost nothing.
fn capabilities(width: u16, height: u16) -> (u16, Vec<u8>) {
    let mut out: Vec<Vec<u8>> = Vec::new();

    // General. No compression of the PDU stream itself, and no fancy update
    // handling -- every flag off is a code path that does not have to exist.
    let mut g = W::default();
    g.le16(1) // osMajorType: Windows, because a server that switches on this
        .le16(3) // osMinorType: Windows NT
        .le16(0x0200) // protocol version, fixed by the standard
        .le16(0)
        .le16(0) // no general compression
        .le16(0) // extra flags
        .le16(0) // update capability
        .le16(0) // remote unshare
        .le16(0) // compression level
        .u8(0) // refreshRect: this console never asks for a repaint
        .u8(0); // suppressOutput
    out.push(cap(1, &g.take()));

    // Bitmap. SIXTEEN BITS, and the desktop size this console wants.
    let mut b = W::default();
    b.le16(16) // preferred bits per pixel
        .le16(1) // receive 1bpp -- required to be 1 by the standard
        .le16(1)
        .le16(1)
        .le16(width)
        .le16(height)
        .le16(0)
        // The desktop may be resized by the server, and saying so is what
        // stops it refusing a size it does not like.
        .le16(1)
        // The standard says this MUST be 1 even for a client that would rather
        // not decompress anything. The server decides per rectangle; the
        // decoder below is the price of that sentence.
        .le16(1)
        .le16(0)
        .le16(0)
        .le16(1) // multiple rectangles in one update
        .le16(0);
    out.push(cap(2, &b.take()));

    // Order. EVERY ORDER UNSUPPORTED, WHICH IS THE POINT: the server then has
    // to send bitmaps, and this console needs no drawing engine. rfb.rs made
    // the same trade by asking for Raw.
    let mut o = W::default();
    o.zeros(16) // terminal descriptor
        .le32(0)
        .le32(0)
        .le32(0)
        .le16(1) // desktop save: not used
        .le16(0x0014) // maximum order level
        .le16(0) // number of fonts
        .le16(0x0002) // NEGOTIATEORDERS_SUPPORT, and nothing else
        .zeros(32) // orderSupport: not one order
        .le16(0) // text flags
        .le16(0) // order support extra
        .le32(0)
        .le32(230400) // desktop save size, the standard's own number
        .le16(0)
        .le16(0)
        .le32(0);
    out.push(cap(3, &o.take()));

    // Bitmap cache, revision 1, all zeroes: no caching. A cache is a second
    // copy of the screen to keep correct.
    let mut c = W::default();
    c.zeros(24).le16(0).le16(0).le16(0).le16(0).le16(0).le16(0);
    out.push(cap(4, &c.take()));

    // Pointer. Colour pointers only, and a cache the server insists exists.
    let mut p = W::default();
    p.le16(1).le16(20).le16(20);
    out.push(cap(8, &p.take()));

    // Share and input.
    let mut sh = W::default();
    sh.le16(0).le16(0);
    out.push(cap(9, &sh.take()));

    let mut inp = W::default();
    inp.le16(0x0001 | 0x0004 | 0x0008 | 0x0020) // scancodes, mousex, unicode, fastpath
        .le16(0)
        .le32(0x0409)
        .le32(0)
        .le32(0)
        .zeros(64) // ime file name
        .le16(0)
        .le16(0);
    out.push(cap(13, &inp.take()));

    // Colour table, sound, font and glyph caches: present because servers
    // refuse a client that omits them, empty because none is used.
    let mut ct = W::default();
    ct.le16(6).le16(0);
    out.push(cap(10, &ct.take()));
    let mut snd = W::default();
    snd.le16(0).le16(0);
    out.push(cap(12, &snd.take()));
    let mut f = W::default();
    f.le16(0x0001).le16(0);
    out.push(cap(14, &f.take()));
    let mut gl = W::default();
    gl.zeros(40).le16(0).le16(0);
    out.push(cap(16, &gl.take()));
    let mut br = W::default();
    br.le32(0);
    out.push(cap(15, &br.take()));
    let mut off = W::default();
    off.le32(0).le16(0).le16(0);
    out.push(cap(17, &off.take()));
    let mut vc = W::default();
    vc.le32(0).le32(0);
    out.push(cap(20, &vc.take()));

    let count = out.len() as u16;
    (count, out.concat())
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

use std::io::{Read, Write};
use std::net::TcpStream;

/// A live desktop: a framebuffer, and somewhere to send keystrokes.
pub struct Session {
    tls: tls::Conn,
    inbuf: Vec<u8>,
    user_id: u16,
    io_channel: u16,
    share_id: u32,
    pub width: u16,
    pub height: u16,
    /// The screen, as `0x00RRGGBB` words -- the same layout `draw::Canvas`
    /// uses, so a pane can blit it without converting anything.
    pub frame: Vec<u32>,
    /// Set when anything was painted since the last look.
    pub dirty: bool,
    /// What the node said about itself, kept for the status line.
    pub cert: Option<x509::Cert>,
    /// How many rectangles have arrived, and how many of those were
    /// compressed. KEPT BECAUSE THE ANSWER DECIDES WHETHER THE RLE DECODER IS
    /// BEING TESTED AT ALL: a server that never compresses leaves two hundred
    /// lines of this file unexercised, and that is worth knowing rather than
    /// assuming.
    pub rects: usize,
    pub compressed: usize,
}

/// Everything that can come back off the wire in one read.
enum Pdu {
    /// An MCS message, still wrapped.
    Slow(Vec<u8>),
    /// A fast-path update, already unwrapped.
    Fast(Vec<u8>),
}

impl Session {
    const TIMEOUT: Duration = Duration::from_secs(20);

    /// The whole ceremony: X.224, TLS, MCS, capabilities, and the first frame.
    pub fn connect(d: &Dial) -> Result<Session, String> {
        let mut sock =
            TcpStream::connect(&d.addr).map_err(|e| format!("cannot reach {}: {}", d.addr, e))?;
        sock.set_nodelay(true).ok();
        sock.set_read_timeout(Some(Self::TIMEOUT)).ok();

        // X.224, in the clear, because the negotiation is what decides whether
        // there will be any TLS at all.
        sock.write_all(&connection_request(&d.user))
            .map_err(|e| format!("writing to {}: {}", d.addr, e))?;
        let confirm = read_tpkt(&mut sock)?;
        connection_confirm(&confirm)?;

        let tls = tls::Conn::start(
            sock,
            &tls::Config {
                host: d.host.clone(),
                cas: d.cas.clone(),
                client: d.client.clone(),
            },
        )?;
        let cert = tls.peer.clone();

        let mut s = Session {
            tls,
            inbuf: Vec::new(),
            user_id: 0,
            io_channel: 0,
            share_id: 0,
            width: d.width,
            height: d.height,
            frame: vec![0; d.width as usize * d.height as usize],
            dirty: false,
            cert,
            rects: 0,
            compressed: 0,
        };
        s.mcs(d)?;
        s.activate(d)?;
        Ok(s)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), String> {
        self.tls.write(data)
    }

    /// One PDU, waiting for it.
    fn pdu(&mut self) -> Result<Pdu, String> {
        let start = Instant::now();
        loop {
            if let Some(p) = self.take_pdu()? {
                return Ok(p);
            }
            if self.tls.ended() {
                return Err("the node closed the desktop".into());
            }
            if start.elapsed() > Self::TIMEOUT {
                return Err("the node stopped sending".into());
            }
            let got = self.tls.read()?;
            if got.is_empty() {
                std::thread::sleep(Duration::from_millis(2));
            } else {
                self.inbuf.extend_from_slice(&got);
            }
        }
    }

    /// A PDU if a whole one is buffered.
    ///
    /// TWO FRAMINGS SHARE ONE STREAM. A slow-path message starts with the TPKT
    /// version byte, 3; a fast-path one starts with an action byte whose low
    /// two bits are zero. Those cannot be confused because 3 has both low bits
    /// set, which is exactly why the standard chose it.
    fn take_pdu(&mut self) -> Result<Option<Pdu>, String> {
        if self.inbuf.len() < 4 {
            return Ok(None);
        }
        if self.inbuf[0] == 3 {
            let len = u16::from_be_bytes([self.inbuf[2], self.inbuf[3]]) as usize;
            if len < 7 || len > 65535 {
                return Err(format!("a TPKT claiming to be {} bytes", len));
            }
            if self.inbuf.len() < len {
                return Ok(None);
            }
            // Past the TPKT header and the X.224 data header.
            let body = self.inbuf[7..len].to_vec();
            self.inbuf.drain(..len);
            return Ok(Some(Pdu::Slow(body)));
        }
        let (len, at) = if self.inbuf[1] & 0x80 != 0 {
            (
                ((self.inbuf[1] & 0x7f) as usize) << 8 | self.inbuf[2] as usize,
                3,
            )
        } else {
            (self.inbuf[1] as usize, 2)
        };
        if len < at {
            return Err("a fast-path update shorter than its own header".into());
        }
        if self.inbuf.len() < len {
            return Ok(None);
        }
        let body = self.inbuf[at..len].to_vec();
        self.inbuf.drain(..len);
        Ok(Some(Pdu::Fast(body)))
    }

    /// Wrap a payload for the I/O channel and send it.
    fn send_data(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut w = W::default();
        w.u8(mcs::SEND_DATA_REQUEST)
            .be16(self.user_id)
            .be16(self.io_channel)
            .u8(0x70); // data priority and segmentation: one whole message
        if payload.len() < 0x80 {
            w.u8(payload.len() as u8);
        } else {
            w.be16(0x8000 | payload.len() as u16);
        }
        w.raw(payload);
        let body = w.take();
        self.send(&tpkt(&x224_data(&body)))
    }

    /// Unwrap an MCS data indication, or say what else it was.
    fn mcs_payload(&mut self, b: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let mut r = R::new(b);
        let kind = r.u8()?;
        match kind {
            mcs::SEND_DATA_INDICATION => {
                let _initiator = r.be16()?;
                let _channel = r.be16()?;
                let _flags = r.u8()?;
                let first = r.u8()?;
                let len = if first & 0x80 != 0 {
                    ((first & 0x7f) as usize) << 8 | r.u8()? as usize
                } else {
                    first as usize
                };
                let data = r.take(len.min(r.rest().len()))?;
                Ok(Some(data.to_vec()))
            }
            mcs::DISCONNECT_PROVIDER => Err("the node ended the session".into()),
            n => Err(format!("an MCS message of type {:#x}", n)),
        }
    }

    fn mcs(&mut self, d: &Dial) -> Result<(), String> {
        self.send(&connect_initial(d))?;
        let response = match self.pdu()? {
            Pdu::Slow(b) => b,
            Pdu::Fast(_) => return Err("a fast-path update before the conference existed".into()),
        };
        let blocks = server_blocks(&response)?;
        for (kind, body) in &blocks {
            if *kind == 0x0c03 {
                // SC_NET: the channel every RDP message rides on.
                let mut r = R::new(body);
                self.io_channel = r.le16()?;
            }
            if *kind == 0x0c02 {
                // SC_SECURITY. With TLS underneath, the only acceptable answer
                // is no RDP encryption: anything else means the node wants to
                // wrap the session in RC4 as well, which profile.rs forbids
                // and this console cannot do.
                let mut r = R::new(body);
                let method = r.le32()?;
                let level = r.le32()?;
                if method != 0 || level > 2 {
                    return Err(format!(
                        "the node wants RDP encryption method {} level {}, and this console \
                         speaks TLS alone",
                        method, level
                    ));
                }
            }
        }
        if self.io_channel == 0 {
            return Err("the node named no channel to talk on".into());
        }

        // Erect domain, attach a user, join the two channels. Four messages
        // that exist because MCS was designed for conference calls.
        self.send(&tpkt(&x224_data(&[mcs::ERECT_DOMAIN, 0x01, 0x00, 0x01, 0x00])))?;
        self.send(&tpkt(&x224_data(&[mcs::ATTACH_USER_REQUEST])))?;
        let confirm = match self.pdu()? {
            Pdu::Slow(b) => b,
            Pdu::Fast(_) => return Err("a fast-path update before there was a user".into()),
        };
        let mut r = R::new(&confirm);
        if r.u8()? != mcs::ATTACH_USER_CONFIRM {
            return Err("the node did not confirm a user".into());
        }
        let result = r.u8()?;
        if result >> 4 != 0 {
            return Err("the node refused to attach a user".into());
        }
        self.user_id = r.be16()? + 1001;

        for channel in [self.user_id, self.io_channel] {
            let mut w = W::default();
            w.u8(mcs::CHANNEL_JOIN_REQUEST)
                .be16(self.user_id - 1001)
                .be16(channel);
            let body = w.take();
            self.send(&tpkt(&x224_data(&body)))?;
            let reply = match self.pdu()? {
                Pdu::Slow(b) => b,
                Pdu::Fast(_) => return Err("a fast-path update mid-join".into()),
            };
            let mut r = R::new(&reply);
            if r.u8()? != mcs::CHANNEL_JOIN_CONFIRM {
                return Err("the node did not confirm a channel".into());
            }
            if r.u8()? >> 4 != 0 {
                return Err(format!("the node refused channel {}", channel));
            }
        }
        Ok(())
    }
}

/// Read one TPKT off a socket that has no TLS on it yet.
fn read_tpkt(s: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut head = [0u8; 4];
    s.read_exact(&mut head)
        .map_err(|e| format!("the node said nothing: {}", e))?;
    if head[0] != 3 {
        return Err("that is not an RDP server".into());
    }
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    if len < 4 || len > 8192 {
        return Err(format!("a TPKT claiming to be {} bytes", len));
    }
    let mut rest = vec![0u8; len - 4];
    s.read_exact(&mut rest)
        .map_err(|e| format!("the node stopped mid-message: {}", e))?;
    let mut out = head.to_vec();
    out.extend_from_slice(&rest);
    Ok(out)
}

impl Session {
    fn share_control(&self, kind: u16, body: &[u8]) -> Vec<u8> {
        let mut w = W::default();
        w.le16((body.len() + 6) as u16)
            .le16(kind)
            .le16(self.user_id)
            .raw(body);
        w.take()
    }

    fn share_data(&self, kind2: u8, body: &[u8]) -> Vec<u8> {
        let mut w = W::default();
        w.le32(self.share_id)
            .u8(0)
            .u8(1) // STREAM_LOW
            // The length the standard wants here is the whole PDU minus
            // fourteen, which is the one arithmetic in RDP that looks like a
            // typo and is not.
            .le16((body.len() + 4) as u16)
            .u8(kind2)
            .u8(0)
            .le16(0)
            .raw(body);
        self.share_control(pdu::DATA, &w.take())
    }

    /// The client info PDU: who is logging in, and what the session should do.
    fn client_info(&mut self, d: &Dial) -> Result<(), String> {
        let mut info = W::default();
        info.le32(0) // code page
            // Mouse, no Ctrl-Alt-Del screen, Unicode, maximised shell, and the
            // Windows key. No INFO_COMPRESSION: a compressed PDU stream is a
            // second decompressor to keep correct.
            .le32(0x0001 | 0x0002 | 0x0010 | 0x0020 | 0x0100)
            .le16((d.domain.encode_utf16().count() * 2) as u16)
            .le16((d.user.encode_utf16().count() * 2) as u16)
            .le16(0) // no password: the fleet authenticates with certificates
            .le16(0)
            .le16(0)
            .utf16(&d.domain)
            .utf16(&d.user)
            .utf16("")
            .utf16("")
            .utf16("")
            // The extended info an RDP5 server expects, all of it empty.
            .le16(2) // AF_INET
            .le16(2)
            .zeros(2)
            .le16(2)
            .zeros(2)
            .zeros(172) // time zone
            .le32(0) // session id
            .le32(0) // performance flags
            .le16(0); // no auto-reconnect cookie
        let body = info.take();

        let mut w = W::default();
        w.le16(0x0040) // SEC_INFO_PKT
            .le16(0)
            .raw(&body);
        let payload = w.take();
        self.send_data(&payload)
    }

    /// Everything from the licence exchange to the first frame.
    fn activate(&mut self, d: &Dial) -> Result<(), String> {
        self.client_info(d)?;

        // The demand-active PDU is what this is waiting for. Anything before
        // it is licensing, which for a fleet node is one message saying there
        // is no licence server and the client may proceed.
        let demand = loop {
            let p = self.pdu()?;
            let body = match p {
                Pdu::Slow(b) => match self.mcs_payload(&b)? {
                    Some(x) => x,
                    None => continue,
                },
                Pdu::Fast(_) => continue,
            };
            match self.share_pdu(&body)? {
                Some((kind, rest)) if kind == pdu::DEMAND_ACTIVE => break rest,
                Some((kind, _)) if kind == pdu::DEACTIVATE_ALL => {
                    return Err("the node deactivated the session before it began".into())
                }
                _ => continue,
            }
        };

        // What the server can do, of which this console reads two numbers.
        let mut r = R::new(&demand);
        self.share_id = r.le32()?;
        let source_len = r.le16()? as usize;
        let _caps_len = r.le16()?;
        r.skip(source_len)?;
        let count = r.le16()?;
        r.skip(2)?;
        for _ in 0..count {
            if r.rest().len() < 4 {
                break;
            }
            let kind = r.le16()?;
            let len = r.le16()? as usize;
            if len < 4 || len - 4 > r.rest().len() {
                break;
            }
            let body = r.take(len - 4)?;
            if kind == 2 {
                // The server's bitmap capability set carries the size of the
                // desktop it is actually going to send.
                let mut b = R::new(body);
                let _bpp = b.le16()?;
                b.skip(6)?;
                let w = b.le16()?;
                let h = b.le16()?;
                if w > 0 && h > 0 && (w != self.width || h != self.height) {
                    self.width = w;
                    self.height = h;
                    self.frame = vec![0; w as usize * h as usize];
                }
            }
        }

        // Confirm, and then the four-message dance that turns a share into a
        // session: synchronise, cooperate, ask for control, send the font list.
        let (count, caps) = capabilities(self.width, self.height);
        let mut w = W::default();
        w.le32(self.share_id)
            .le16(0x03ea) // originator: the standard's magic number for a client
            .le16(6)
            .le16((caps.len() + 4) as u16)
            .raw(b"MSTSC\0")
            .le16(count)
            .le16(0)
            .raw(&caps);
        let body = w.take();
        let msg = self.share_control(pdu::CONFIRM_ACTIVE, &body);
        self.send_data(&msg)?;

        let mut s = W::default();
        s.le16(1).le16(self.io_channel);
        let msg = self.share_data(pdu::SYNCHRONIZE, &s.take());
        self.send_data(&msg)?;

        for action in [4u16, 1u16] {
            let mut c = W::default();
            c.le16(action).le16(0).le32(0);
            let msg = self.share_data(pdu::CONTROL, &c.take());
            self.send_data(&msg)?;
        }

        let mut f = W::default();
        f.le16(0).le16(0).le16(0x0003).le16(50);
        let msg = self.share_data(pdu::FONT_LIST, &f.take());
        self.send_data(&msg)?;

        // The font map ends the handshake. Updates may already be arriving,
        // and they are handled on the way past rather than dropped.
        let start = Instant::now();
        loop {
            if start.elapsed() > Self::TIMEOUT {
                return Err("the node never finished activating the session".into());
            }
            let p = self.pdu()?;
            if self.handle(p)? {
                break;
            }
        }
        Ok(())
    }

    /// Split a share control PDU into its type and the rest.
    fn share_pdu(&self, b: &[u8]) -> Result<Option<(u16, Vec<u8>)>, String> {
        if b.len() < 6 {
            return Ok(None);
        }
        let kind = u16::from_le_bytes([b[2], b[3]]);
        // A security header is four bytes with nothing that looks like a
        // share control type in it. Telling them apart by the type field
        // rather than by the flags is what keeps a licensing message from
        // being read as a capability exchange.
        if kind & 0x0010 == 0 || !matches!(kind & 0x000f, 1 | 3 | 6 | 7) {
            if b.len() > 4 {
                return self.share_pdu(&b[4..]);
            }
            return Ok(None);
        }
        Ok(Some((kind, b[6..].to_vec())))
    }
}

impl Session {
    /// Deal with one PDU. Returns true when it was the font map, which is the
    /// message that means the session is live.
    fn handle(&mut self, p: Pdu) -> Result<bool, String> {
        match p {
            Pdu::Fast(body) => {
                self.fast_updates(&body)?;
                Ok(false)
            }
            Pdu::Slow(b) => {
                let payload = match self.mcs_payload(&b)? {
                    Some(x) => x,
                    None => return Ok(false),
                };
                let (kind, rest) = match self.share_pdu(&payload)? {
                    Some(x) => x,
                    None => return Ok(false),
                };
                if kind == pdu::DEACTIVATE_ALL {
                    return Err("the node deactivated the session".into());
                }
                if kind != pdu::DATA {
                    return Ok(false);
                }
                if rest.len() < 12 {
                    return Ok(false);
                }
                let kind2 = rest[8];
                let body = &rest[12..];
                match kind2 {
                    pdu::UPDATE => {
                        let mut r = R::new(body);
                        let what = r.le16()?;
                        if what == 1 {
                            self.bitmaps(r.rest())?;
                        }
                        Ok(false)
                    }
                    pdu::FONT_MAP => Ok(true),
                    pdu::ERROR_INFO => {
                        let mut r = R::new(body);
                        let code = r.le32()?;
                        if code == 0 {
                            return Ok(false);
                        }
                        Err(format!("the node reported error {}", code))
                    }
                    _ => Ok(false),
                }
            }
        }
    }

    /// A fast-path update carries several updates in one message.
    fn fast_updates(&mut self, b: &[u8]) -> Result<(), String> {
        let mut r = R::new(b);
        while !r.done() {
            if r.rest().len() < 3 {
                break;
            }
            let header = r.u8()?;
            let code = header & 0x0f;
            let compressed = (header >> 6) & 0x02 != 0;
            if compressed {
                // The PDU stream's own compression, which this console never
                // asked for -- INFO_COMPRESSION is off in the client info PDU.
                return Err("the node compressed the update stream, which was never offered".into());
            }
            let size = r.le16()? as usize;
            let data = r.take(size.min(r.rest().len()))?;
            match code {
                // 1 is a bitmap update; everything else is a pointer, a
                // palette or a synchronise, and none of those moves a pixel
                // this console draws.
                1 => {
                    let mut d = R::new(data);
                    let _update_type = d.le16()?;
                    self.bitmaps(d.rest())?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The rectangles in one bitmap update.
    fn bitmaps(&mut self, b: &[u8]) -> Result<(), String> {
        let mut r = R::new(b);
        let count = r.le16()?;
        for _ in 0..count {
            let left = r.le16()?;
            let top = r.le16()?;
            let right = r.le16()?;
            let bottom = r.le16()?;
            let width = r.le16()? as usize;
            let height = r.le16()? as usize;
            let bpp = r.le16()?;
            let flags = r.le16()?;
            let len = r.le16()? as usize;
            let mut data = r.take(len.min(r.rest().len()))?;

            if bpp != 16 {
                // The capability set asked for sixteen. A server that sends
                // something else is a server this console cannot draw, and
                // guessing would paint noise.
                return Err(format!("the node sent {}-bit pixels, and this asked for 16", bpp));
            }
            let compressed = flags & 0x0001 != 0;
            self.rects += 1;
            if compressed {
                self.compressed += 1;
            }
            if compressed && flags & 0x0400 == 0 {
                // The eight-byte compression header, which carries a length
                // this console does not need because the outer length already
                // said how much there is.
                if data.len() < 8 {
                    return Err("a compressed bitmap with no header".into());
                }
                data = &data[8..];
            }
            let pixels = if compressed {
                rle16(data, width, height)?
            } else {
                if data.len() < width * height * 2 {
                    return Err("a bitmap shorter than the rectangle it claims".into());
                }
                data[..width * height * 2]
                    .chunks(2)
                    .map(|p| u16::from_le_bytes([p[0], p[1]]))
                    .collect()
            };
            self.blit(left, top, right, bottom, width, height, &pixels);
        }
        self.dirty = true;
        Ok(())
    }

    /// Put one decoded rectangle into the framebuffer.
    ///
    /// UPSIDE DOWN, BECAUSE RDP BITMAPS ARE. Like a .BMP file, the first row
    /// on the wire is the bottom row on the screen -- for compressed data as
    /// well, because the compression runs over the same bottom-up image.
    fn blit(
        &mut self,
        left: u16,
        top: u16,
        right: u16,
        bottom: u16,
        width: usize,
        height: usize,
        pixels: &[u16],
    ) {
        let _ = (right, bottom);
        for row in 0..height {
            let y = top as usize + (height - 1 - row);
            if y >= self.height as usize {
                continue;
            }
            for col in 0..width {
                let x = left as usize + col;
                if x >= self.width as usize {
                    continue;
                }
                let p = match pixels.get(row * width + col) {
                    Some(p) => *p,
                    None => continue,
                };
                self.frame[y * self.width as usize + x] = rgb565(p);
            }
        }
    }

    /// Read whatever has arrived and paint it. Never blocks.
    pub fn poll(&mut self) -> Result<bool, String> {
        let got = self.tls.read()?;
        if !got.is_empty() {
            self.inbuf.extend_from_slice(&got);
        }
        let before = self.dirty;
        while let Some(p) = self.take_pdu()? {
            self.handle(p)?;
        }
        let changed = self.dirty && !before;
        Ok(changed || self.dirty)
    }

    /// Ask the node to repaint the whole screen.
    ///
    /// Used when a pane is first shown: a session that joins an idle desktop
    /// sees nothing at all until something moves, which looks like a broken
    /// connection rather than a still one.
    pub fn refresh(&mut self) -> Result<(), String> {
        let mut w = W::default();
        w.u8(1) // one rectangle
            .zeros(1)
            .le16(0)
            .le16(0)
            .le16(self.width.saturating_sub(1))
            .le16(self.height.saturating_sub(1));
        let msg = self.share_data(33, &w.take()); // PDUTYPE2_REFRESH_RECT
        self.send_data(&msg)
    }

    /// A key, by its set-1 scancode -- which is exactly what `keymap.rs`
    /// produces, and the reason it produces it.
    pub fn key(&mut self, scancode: u16, down: bool) -> Result<(), String> {
        let extended = scancode & 0xff00 == 0xe000;
        let mut flags = 0u8;
        if !down {
            flags |= 0x01; // release
        }
        if extended {
            flags |= 0x02;
        }
        let mut e = W::default();
        e.u8(flags).u8((scancode & 0xff) as u8);
        self.input(&[e.take()])
    }

    /// The pointer, in the same vocabulary `seat.rs` already speaks.
    pub fn pointer(&mut self, x: u16, y: u16, buttons: u8, wheel: i8) -> Result<(), String> {
        let mut flags = 0x0800u16; // move
        if buttons & 1 != 0 {
            flags |= 0x8000 | 0x1000;
        } else if buttons & 2 != 0 {
            flags |= 0x8000 | 0x2000;
        } else if buttons & 4 != 0 {
            flags |= 0x8000 | 0x4000;
        }
        let mut e = W::default();
        e.u8(1 << 5).le16(flags).le16(x).le16(y);
        let mut events = vec![e.take()];
        if wheel != 0 {
            let mut w = W::default();
            let rotation = (wheel as i16 * 120) as u16 & 0x01ff;
            let negative = if wheel < 0 { 0x0100 } else { 0 };
            w.u8(1 << 5)
                .le16(0x0200 | negative | rotation)
                .le16(x)
                .le16(y);
            events.push(w.take());
        }
        self.input(&events)
    }

    fn input(&mut self, events: &[Vec<u8>]) -> Result<(), String> {
        let body: Vec<u8> = events.concat();
        // The fast-path input header: the action is zero, the event count
        // lives in the top bits, and the length counts itself.
        let mut out = Vec::new();
        let n = events.len() as u8;
        out.push((n & 0x0f) << 2);
        let total = body.len() + 3;
        out.push(0x80 | (total >> 8) as u8);
        out.push(total as u8);
        out.extend_from_slice(&body);
        self.send(&out)
    }

    pub fn close(&mut self) {
        self.tls.close();
    }
}

/// A 16-bit RGB565 pixel as the canvas's `0x00RRGGBB`.
///
/// The five- and six-bit fields are scaled rather than shifted: shifting
/// leaves the brightest colour at 0xF8 rather than 0xFF, so a white desktop
/// arrives faintly grey and every screenshot is subtly wrong.
fn rgb565(p: u16) -> u32 {
    let r = ((p >> 11) & 0x1f) as u32;
    let g = ((p >> 5) & 0x3f) as u32;
    let b = (p & 0x1f) as u32;
    let r = (r * 255 + 15) / 31;
    let g = (g * 255 + 31) / 63;
    let b = (b * 255 + 15) / 31;
    r << 16 | g << 8 | b
}

// ---------------------------------------------------------------------------
// Interleaved RLE -- MS-RDPBCGR 3.1.9
// ---------------------------------------------------------------------------
//
// WHY THIS HAD TO BE WRITTEN AT ALL. The bitmap capability set's compression
// flag "MUST be set to TRUE", says the standard, even for a client that would
// rather receive raw pixels -- the server decides per rectangle. So a client
// that cannot decompress is a client that works until the first server that
// bothers to compress, which is all of them.
//
// THE SHAPE OF IT: a run-length scheme where most runs are a delta against the
// scanline above. A foreground run writes `fgPel XOR the pixel above`; a
// background run copies the pixel above unchanged. That is why the first
// scanline has its own rules -- there is nothing above it -- and why an error
// anywhere propagates downwards through the whole rectangle rather than
// showing up as one wrong pixel.
//
// IT IS CHECKED BY ARITHMETIC RATHER THAN BY EYE. `rle16` refuses to write
// past the rectangle, refuses to read past the input, and the caller requires
// that the output was filled exactly. A decoder that is subtly wrong almost
// always ends up short or long, so "it consumed everything and filled
// everything" is a real check and not a comfortable one.

/// Which code a header byte carries, in the standard's own numbering.
fn rle_code(h: u8) -> u8 {
    if h & 0xc0 != 0xc0 {
        h >> 5 // regular: 000x xxxx through 100x xxxx
    } else if h & 0xf0 == 0xf0 {
        h // mega-mega and the specials
    } else {
        h >> 4 // lite: 1100 xxxx, 1101 xxxx, 1110 xxxx
    }
}

const BG_RUN: u8 = 0;
const FG_RUN: u8 = 1;
const FGBG_IMAGE: u8 = 2;
const COLOR_RUN: u8 = 3;
const COLOR_IMAGE: u8 = 4;
const LITE_SET_FG_FG_RUN: u8 = 0x0c;
const LITE_SET_FG_FGBG_IMAGE: u8 = 0x0d;
const LITE_DITHERED_RUN: u8 = 0x0e;
const MEGA_BG_RUN: u8 = 0xf0;
const MEGA_FG_RUN: u8 = 0xf1;
const MEGA_FGBG_IMAGE: u8 = 0xf2;
const MEGA_COLOR_RUN: u8 = 0xf3;
const MEGA_COLOR_IMAGE: u8 = 0xf4;
const MEGA_SET_FG_RUN: u8 = 0xf6;
const MEGA_SET_FGBG_IMAGE: u8 = 0xf7;
const MEGA_DITHERED_RUN: u8 = 0xf8;
const SPECIAL_FGBG_1: u8 = 0xf9;
const SPECIAL_FGBG_2: u8 = 0xfa;
const WHITE: u8 = 0xfd;
const BLACK: u8 = 0xfe;

/// Decompress one rectangle of 16-bit pixels.
pub fn rle16(src: &[u8], width: usize, height: usize) -> Result<Vec<u16>, String> {
    let total = width
        .checked_mul(height)
        .ok_or("a rectangle larger than arithmetic")?;
    let mut out = vec![0u16; total];
    let mut at = 0usize;
    let mut pos = 0usize;
    let mut fg: u16 = 0xffff;
    let mut insert_fg = false;
    let mut first_line = true;

    // Every write goes through here, so there is one bounds check rather than
    // twenty.
    macro_rules! put {
        ($v:expr) => {{
            if pos >= total {
                return Err("the compressed bitmap is larger than its own rectangle".into());
            }
            out[pos] = $v;
            pos += 1;
        }};
    }
    macro_rules! above {
        () => {{
            if pos < width {
                0u16
            } else {
                out[pos - width]
            }
        }};
    }
    macro_rules! pixel {
        () => {{
            if at + 2 > src.len() {
                return Err("the compressed bitmap ended inside a pixel".into());
            }
            let v = u16::from_le_bytes([src[at], src[at + 1]]);
            at += 2;
            v
        }};
    }
    macro_rules! byte {
        () => {{
            if at >= src.len() {
                return Err("the compressed bitmap ended inside a run".into());
            }
            let v = src[at];
            at += 1;
            v
        }};
    }

    while at < src.len() && pos < total {
        if first_line && pos >= width {
            first_line = false;
            insert_fg = false;
        }
        let header = src[at];
        let code = rle_code(header);

        // The run length, which is spelled four different ways depending on
        // how big it is and which code it belongs to.
        let mut run: usize;
        match code {
            FGBG_IMAGE | LITE_SET_FG_FGBG_IMAGE => {
                let mask = if code == FGBG_IMAGE { 0x1f } else { 0x0f };
                run = (header & mask) as usize;
                at += 1;
                if run == 0 {
                    run = byte!() as usize + 1;
                } else {
                    run *= 8;
                }
            }
            BG_RUN | FG_RUN | COLOR_RUN | COLOR_IMAGE => {
                run = (header & 0x1f) as usize;
                at += 1;
                if run == 0 {
                    run = byte!() as usize + 32;
                }
            }
            LITE_SET_FG_FG_RUN | LITE_DITHERED_RUN => {
                run = (header & 0x0f) as usize;
                at += 1;
                if run == 0 {
                    run = byte!() as usize + 16;
                }
            }
            MEGA_BG_RUN | MEGA_FG_RUN | MEGA_FGBG_IMAGE | MEGA_COLOR_RUN | MEGA_COLOR_IMAGE
            | MEGA_SET_FG_RUN | MEGA_SET_FGBG_IMAGE | MEGA_DITHERED_RUN => {
                at += 1;
                let lo = byte!() as usize;
                let hi = byte!() as usize;
                run = lo | hi << 8;
            }
            SPECIAL_FGBG_1 | SPECIAL_FGBG_2 | WHITE | BLACK => {
                at += 1;
                run = 0;
            }
            n => return Err(format!("an RLE code of {:#x}, which is not one", n)),
        }

        match code {
            BG_RUN | MEGA_BG_RUN => {
                // A background run that follows another run starts with one
                // foreground pixel. That rule is the whole reason `insert_fg`
                // exists, and forgetting it shifts every subsequent pixel.
                if insert_fg && run > 0 {
                    let v = if first_line { fg } else { above!() ^ fg };
                    put!(v);
                    run -= 1;
                }
                for _ in 0..run {
                    let v = if first_line { 0 } else { above!() };
                    put!(v);
                }
                insert_fg = true;
                continue;
            }
            FG_RUN | MEGA_FG_RUN | LITE_SET_FG_FG_RUN | MEGA_SET_FG_RUN => {
                if code == LITE_SET_FG_FG_RUN || code == MEGA_SET_FG_RUN {
                    fg = pixel!();
                }
                for _ in 0..run {
                    let v = if first_line { fg } else { above!() ^ fg };
                    put!(v);
                }
            }
            LITE_DITHERED_RUN | MEGA_DITHERED_RUN => {
                let a = pixel!();
                let b = pixel!();
                for _ in 0..run {
                    put!(a);
                    put!(b);
                }
            }
            COLOR_RUN | MEGA_COLOR_RUN => {
                let a = pixel!();
                for _ in 0..run {
                    put!(a);
                }
            }
            COLOR_IMAGE | MEGA_COLOR_IMAGE => {
                for _ in 0..run {
                    let a = pixel!();
                    put!(a);
                }
            }
            FGBG_IMAGE | MEGA_FGBG_IMAGE | LITE_SET_FG_FGBG_IMAGE | MEGA_SET_FGBG_IMAGE => {
                if code == LITE_SET_FG_FGBG_IMAGE || code == MEGA_SET_FGBG_IMAGE {
                    fg = pixel!();
                }
                let mut left = run;
                while left > 8 {
                    let mask = byte!();
                    for bit in 0..8 {
                        let v = fgbg(mask, bit, fg, first_line, above!());
                        put!(v);
                    }
                    left -= 8;
                }
                if left > 0 {
                    let mask = byte!();
                    for bit in 0..left {
                        let v = fgbg(mask, bit, fg, first_line, above!());
                        put!(v);
                    }
                }
            }
            SPECIAL_FGBG_1 | SPECIAL_FGBG_2 => {
                // Two bit patterns common enough to have their own codes.
                let mask = if code == SPECIAL_FGBG_1 { 0x03 } else { 0x05 };
                for bit in 0..8 {
                    let v = fgbg(mask, bit, fg, first_line, above!());
                    put!(v);
                }
            }
            WHITE => put!(0xffff),
            BLACK => put!(0),
            _ => unreachable!(),
        }
        insert_fg = false;
    }

    if pos != total {
        return Err(format!(
            "the compressed bitmap filled {} pixels of {}",
            pos, total
        ));
    }
    Ok(out)
}

fn fgbg(mask: u8, bit: usize, fg: u16, first_line: bool, above: u16) -> u16 {
    let set = mask >> bit & 1 == 1;
    match (set, first_line) {
        (true, true) => fg,
        (false, true) => 0,
        (true, false) => above ^ fg,
        (false, false) => above,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }

    #[test]
    fn a_colour_run_fills_what_it_says() {
        let mut s = vec![0x60 | 4];
        s.extend_from_slice(&px(0x1234));
        assert_eq!(rle16(&s, 4, 1).unwrap(), vec![0x1234; 4]);
    }

    #[test]
    fn a_background_run_copies_the_line_above() {
        // THE WHOLE IDEA OF THE FORMAT, in eight bytes: the second scanline
        // says "the same as the one above" and costs one byte.
        let mut s = vec![0x60 | 4];
        s.extend_from_slice(&px(0xabcd));
        s.push(0x04); // background run of four
        let out = rle16(&s, 4, 2).unwrap();
        assert_eq!(&out[..4], &[0xabcd; 4]);
        assert_eq!(&out[4..], &[0xabcd; 4], "the second line did not copy the first");
    }

    #[test]
    fn a_foreground_run_is_a_difference_from_the_line_above() {
        let mut s = vec![0x60 | 2];
        s.extend_from_slice(&px(0x00ff));
        // Lite set-foreground run of two: sets the foreground and XORs it.
        s.push(0xc0 | 2);
        s.extend_from_slice(&px(0x0f0f));
        let out = rle16(&s, 2, 2).unwrap();
        assert_eq!(&out[..2], &[0x00ff, 0x00ff]);
        assert_eq!(&out[2..], &[0x00ff ^ 0x0f0f, 0x00ff ^ 0x0f0f]);
    }

    #[test]
    fn white_and_black_are_one_pixel_each() {
        let s = vec![WHITE, BLACK, WHITE, BLACK];
        assert_eq!(rle16(&s, 4, 1).unwrap(), vec![0xffff, 0, 0xffff, 0]);
    }

    #[test]
    fn a_colour_image_is_raw_pixels() {
        let mut s = vec![0x80 | 3];
        for v in [1u16, 2, 3] {
            s.extend_from_slice(&px(v));
        }
        assert_eq!(rle16(&s, 3, 1).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn a_foreground_background_image_reads_its_mask_from_the_bottom_bit_up() {
        // One mask byte, eight pixels, on the first line: a set bit is the
        // foreground and a clear bit is black.
        let mut s = vec![0xd0 | 1]; // lite set-fg fgbg image, eight pixels
        s.extend_from_slice(&px(0x7777));
        s.push(0b1010_0101);
        let out = rle16(&s, 8, 1).unwrap();
        let f = 0x7777;
        assert_eq!(out, vec![f, 0, f, 0, 0, f, 0, f]);
    }

    #[test]
    fn a_stream_that_does_not_fill_the_rectangle_is_refused() {
        // A DECODER THAT IS SUBTLY WRONG ENDS UP SHORT OR LONG, so this is the
        // check that makes "it decoded without complaining" mean something.
        let mut s = vec![0x60 | 2];
        s.extend_from_slice(&px(1));
        let e = rle16(&s, 8, 1).unwrap_err();
        assert!(e.contains("filled 2 pixels of 8"), "{}", e);
    }

    #[test]
    fn a_stream_that_overflows_the_rectangle_is_refused() {
        let mut s = vec![0x60 | 20];
        s.extend_from_slice(&px(1));
        let e = rle16(&s, 4, 1).unwrap_err();
        assert!(e.contains("larger than its own rectangle"), "{}", e);
    }

    #[test]
    fn a_truncated_stream_is_a_sentence_rather_than_a_panic() {
        // The first thing a hostile or broken server sends.
        assert!(rle16(&[0x60], 4, 1).is_err());
        assert!(rle16(&[0x60, 0x00], 4, 1).is_err());
        assert!(rle16(&[0xf3], 4, 1).is_err());
        assert!(rle16(&[0xff], 4, 1).is_err());
        assert!(rle16(&[], 4, 1).is_err());
    }

    #[test]
    fn sixteen_bit_pixels_reach_the_ends_of_the_scale() {
        // A shift rather than a scale leaves white at 0xf8f8f8, and every
        // screenshot comes out faintly grey.
        assert_eq!(rgb565(0xffff), 0x00ff_ffff);
        assert_eq!(rgb565(0x0000), 0);
        assert_eq!(rgb565(0xf800), 0x00ff_0000, "pure red");
        assert_eq!(rgb565(0x07e0), 0x0000_ff00, "pure green");
        assert_eq!(rgb565(0x001f), 0x0000_00ff, "pure blue");
    }

    #[test]
    fn the_negotiation_asks_for_tls_and_nothing_else() {
        let req = connection_request("copal");
        assert_eq!(req[0], 3, "not a TPKT");
        assert_eq!(req[1], 0);
        assert_eq!(
            u16::from_be_bytes([req[2], req[3]]) as usize,
            req.len(),
            "the TPKT length does not match the message"
        );
        // The negotiation block is the last eight bytes: type, flags, length,
        // and the protocol mask.
        let n = req.len();
        assert_eq!(req[n - 8], 0x01, "not a negotiation request");
        assert_eq!(u16::from_le_bytes([req[n - 6], req[n - 5]]), 8);
        assert_eq!(
            u32::from_le_bytes([req[n - 4], req[n - 3], req[n - 2], req[n - 1]]),
            PROTOCOL_SSL,
            "this console must ask for TLS alone -- not RDP's own RC4, not CredSSP"
        );
        assert_eq!(PROTOCOL_SSL, crate::profile::P1.rdp_protocol);
    }

    #[test]
    fn every_way_the_node_can_say_no_is_a_sentence() {
        let confirm = |body: Vec<u8>| {
            let mut out = vec![3u8, 0];
            out.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
            out.extend_from_slice(&body);
            connection_confirm(&out)
        };
        // A plain confirm with TLS selected.
        let ok = confirm(vec![6, 0xd0, 0, 0, 0, 0, 0, 0x02, 0, 8, 0, 1, 0, 0, 0]);
        assert!(ok.is_ok(), "{:?}", ok);
        // The same, with NLA demanded instead.
        let e = confirm(vec![6, 0xd0, 0, 0, 0, 0, 0, 0x03, 0, 8, 0, 1, 0, 0, 0]).unwrap_err();
        assert!(e.contains("NLA"), "{}", e);
        assert!(e.contains("CredSSP"), "{}", e);
        // And a server with no negotiation response at all, which means it
        // only speaks RDP's own broken security.
        let e = confirm(vec![6, 0xd0, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(e.contains("no TLS"), "{}", e);
    }

    #[test]
    fn the_conference_request_carries_the_three_blocks_a_server_looks_for() {
        let d = Dial {
            addr: "127.0.0.1:3389".into(),
            host: "museum-01".into(),
            user: "copal".into(),
            domain: String::new(),
            cas: Vec::new(),
            client: None,
            width: 1024,
            height: 768,
        };
        let msg = connect_initial(&d);
        // The H.221 key that says "a client's data follows".
        assert!(
            msg.windows(4).any(|w| w == b"Duca"),
            "no client H.221 key in the conference request"
        );
        // CS_CORE, CS_SECURITY, CS_NET, in that order -- searched for AFTER
        // the H.221 key, because the PER prologue itself contains the bytes
        // `01 c0` and a naive search finds those first.
        let start = msg.windows(4).position(|w| w == b"Duca").unwrap();
        let find = |k: u16| {
            msg[start..]
                .windows(2)
                .position(|w| w == k.to_le_bytes())
                .map(|i| i + start)
        };
        let core = find(0xc001).expect("no CS_CORE");
        let sec = find(0xc002).expect("no CS_SECURITY");
        let net = find(0xc003).expect("no CS_NET");
        assert!(core < sec && sec < net, "the client blocks are out of order");
        // The desktop size, where the server expects to read it.
        assert_eq!(u16::from_le_bytes([msg[core + 8], msg[core + 9]]), 1024);
        assert_eq!(u16::from_le_bytes([msg[core + 10], msg[core + 11]]), 768);
        // And no RDP encryption methods, because TLS is underneath.
        assert_eq!(&msg[sec + 4..sec + 12], &[0u8; 8]);
    }

    #[test]
    fn the_server_blocks_are_found_by_their_own_key() {
        let mut b = vec![0u8; 20];
        b.extend_from_slice(b"McDn");
        b.push(0x14); // a short PER length
        b.extend_from_slice(&0x0c01u16.to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&[1, 2, 3, 4]);
        b.extend_from_slice(&0x0c03u16.to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&1003u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        let blocks = server_blocks(&b).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, 0x0c01);
        assert_eq!(blocks[1].0, 0x0c03);
        assert_eq!(u16::from_le_bytes([blocks[1].1[0], blocks[1].1[1]]), 1003);
        assert!(server_blocks(b"nothing in here").is_err());
    }
}

/// The tests that need a real RDP server.
///
/// R1 AND R2 IN `docs/wire.md` SAY NOTHING HERE HAS EVER TALKED TO hypr-rdp,
/// and that is still true. What these prove is that this client agrees with a
/// server written by other people -- FreeRDP's own sample server, which speaks
/// the same MS-RDPBCGR a node's server will. The remaining risk is named
/// rather than papered over: what hypr-rdp negotiates is still unknown, and
/// finding out is the first thing to do with a node.
#[cfg(test)]
mod live {
    use super::*;
    use crate::x509;

    fn dial() -> Option<Dial> {
        let addr = std::env::var("ORRERY_RDP_ADDR").ok()?;
        let ca = std::fs::read_to_string(std::env::var("ORRERY_RDP_CA").ok()?).ok()?;
        Some(Dial {
            addr,
            host: std::env::var("ORRERY_RDP_HOST").unwrap_or_else(|_| "museum-01".into()),
            user: "copal".into(),
            domain: String::new(),
            cas: x509::from_pem(&ca).ok()?.iter().map(|c| c.key).collect(),
            client: None,
            width: 1024,
            height: 768,
        })
    }

    #[test]
    fn a_real_server_hands_over_a_desktop() {
        let Some(d) = dial() else { return };
        let mut s = Session::connect(&d).expect("the session should open");
        assert!(s.width >= 640 && s.height >= 480, "{}x{}", s.width, s.height);
        assert_eq!(s.frame.len(), s.width as usize * s.height as usize);
        assert!(s.cert.is_some(), "the node's certificate was not kept");

        // Ask for a repaint and wait for pixels. AN IDLE DESKTOP SENDS NOTHING
        // until something moves, which is why the refresh exists at all.
        s.refresh().unwrap();
        let start = Instant::now();
        let mut painted = false;
        while start.elapsed() < Duration::from_secs(15) {
            s.poll().unwrap();
            if s.frame.iter().any(|p| *p != 0) {
                painted = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(painted, "no pixel ever arrived");
        assert!(s.rects > 0, "no rectangle arrived");
        println!(
            "rdp: {}x{}, {} rectangles, {} of them compressed",
            s.width, s.height, s.rects, s.compressed
        );
        s.close();
    }

    #[test]
    fn a_real_server_takes_input() {
        let Some(d) = dial() else { return };
        let mut s = Session::connect(&d).unwrap();
        // A key press and release by set-1 scancode -- `keymap.rs` produces
        // exactly these numbers, which is why it produces them.
        s.key(0x1e, true).unwrap(); // 'a'
        s.key(0x1e, false).unwrap();
        s.key(0xe048, true).unwrap(); // the up arrow, which is extended
        s.key(0xe048, false).unwrap();
        s.pointer(100, 100, 0, 0).unwrap();
        s.pointer(100, 100, 1, 0).unwrap();
        s.pointer(100, 100, 0, 0).unwrap();
        // The session has to survive all of that, which it only does if the
        // fast-path framing was right.
        s.poll().unwrap();
        s.close();
    }

    #[test]
    fn a_server_whose_certificate_is_not_the_fleets_is_refused() {
        let Some(mut d) = dial() else { return };
        let (_, other) = crate::crypto::ed25519_keypair().unwrap();
        d.cas = vec![other];
        let e = match Session::connect(&d) {
            Ok(_) => panic!("a desktop with the wrong certificate was opened"),
            Err(e) => e,
        };
        assert!(e.contains("does not know"), "unhelpful: {}", e);
    }
}
