//! RFB 3.8 -- the protocol behind Control and Observe.
//!
//! §III-B of `docs/fleet-lab-report.md` is where this comes from, and the
//! sentence that matters is the asymmetry: "screenshots for the seven, VNC for
//! the one being controlled". A wall of eight live sessions was priced and
//! refused; ONE session, aimed at the node the operator is sitting in front
//! of, is the design rather than a departure from it.
//!
//! THIS IS THE ONE PLACE IN ORRERY THAT OPENS A SOCKET TO A NODE, and that is
//! deliberate rather than a crack in the rule. The rule in §12 is about the
//! READ MODEL: `copal fleet state` is the only way this program learns what
//! exists, and nothing here changes that -- the address dialled below came out
//! of that document and nowhere else. What the lab report's §D.5 asks for is a
//! live console "on the object itself", and it names the two: the SSH session
//! and the VNC session. A session is not a second way of knowing things; it is
//! the thing being known, looked at directly.
//!
//! NO DEPENDENCIES, STILL. RFB is a small protocol and the subset a seat needs
//! is smaller: the 3.8 handshake, one security type, a pixel format we choose
//! ourselves, and two encodings. Raw is a memcpy and CopyRect is a blit. The
//! compressed encodings (Tight, ZRLE) all want zlib, which is a dependency or
//! six hundred lines of inflate, and on a LAN carrying one session neither is
//! worth it -- so this asks for Raw and says so.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The port x11vnc and wayvnc land on for display :0.
pub const DEFAULT_PORT: u16 = 5900;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Long enough to cross a LAN and decode a full frame; short enough that a
/// node which has stopped answering does not hang the seat.
const IO_TIMEOUT: Duration = Duration::from_secs(20);

/// A frame's worth of bytes, capped. A 1920x1080 screen at 4 bytes a pixel is
/// 8 MB, and anything claiming more than this is a server we do not believe.
const MAX_FRAMEBUFFER: usize = 64 * 1024 * 1024;
/// Nothing legitimate sends a rectangle larger than the screen it lives on,
/// but the count arrives before the geometry does, so it gets its own bound.
const MAX_RECTS: u16 = 4096;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error(e.to_string())
    }
}

fn fail<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

// ----------------------------------------------------------------- pixels ---

/// The screen, as 8-bit RGB. One byte per channel, three per pixel.
///
/// The wire format is negotiated to match this exactly (see `PIXEL_FORMAT`),
/// so decoding a Raw rectangle is a copy with a stride change and never a
/// conversion. Keeping the in-memory picture in the terminal's own colour
/// space is what lets `sixel.rs` stay a pure encoder.
pub struct Screen {
    pub w: usize,
    pub h: usize,
    /// `w * h * 3` bytes, row-major, no padding.
    pub px: Vec<u8>,
}

impl Screen {
    pub fn new(w: usize, h: usize) -> Screen {
        Screen { w, h, px: vec![0; w * h * 3] }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.w + x) * 3;
        (self.px[i], self.px[i + 1], self.px[i + 2])
    }

    #[inline]
    fn put(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        let i = (y * self.w + x) * 3;
        self.px[i] = r;
        self.px[i + 1] = g;
        self.px[i + 2] = b;
    }

    fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.px = vec![0; w * h * 3];
    }
}

// --------------------------------------------------------------- handshake ---

/// Security types, as the RFB registry numbers them.
const SEC_NONE: u8 = 1;
const SEC_VNC_AUTH: u8 = 2;

/// What the server said it is, kept for the header line the seat draws.
pub struct ServerInfo {
    pub name: String,
    pub w: usize,
    pub h: usize,
}

pub struct Rfb {
    /// The read half. Owned by whoever is pumping frames, which in the seat is
    /// a thread of its own -- because `pump` blocks until the node has
    /// something to say, and a still gallery screen says nothing for minutes.
    sock: TcpStream,
    /// The write half, shared. Frame requests come from the pump thread and
    /// key and pointer events come from the input thread; both are RFB client
    /// messages on one socket, so a half-written PointerEvent interleaved into
    /// a FramebufferUpdateRequest would desynchronise the server permanently.
    /// One mutex, held only for the length of a `write_all`, removes that.
    tx: Arc<Mutex<TcpStream>>,
    pub info: ServerInfo,
    pub screen: Screen,
    /// Set once the server has answered at least one update request, so the
    /// seat can tell "connected" from "connected and has drawn something".
    pub painted: bool,
}

/// The writing end, cloneable and sendable between threads.
///
/// Control's input path is this and nothing else -- it cannot read the screen,
/// which is the property that lets Observe hand one of these out and be sure
/// the mode really is read-only.
#[derive(Clone)]
pub struct Input {
    tx: Arc<Mutex<TcpStream>>,
}

fn send(tx: &Arc<Mutex<TcpStream>>, msg: &[u8]) -> Result<(), Error> {
    match tx.lock() {
        Ok(mut s) => {
            s.write_all(msg)?;
            Ok(())
        }
        // A poisoned lock means the other thread panicked mid-write and the
        // stream's position is unknown, so the session is over.
        Err(_) => fail("the session's writer failed"),
    }
}

/// Read exactly `n` bytes or say why not. `read_exact` already does this; the
/// wrapper exists to turn an EOF into a sentence about RFB rather than about
/// file descriptors, because that is what the operator will read.
fn read_n(sock: &mut TcpStream, n: usize) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; n];
    match sock.read_exact(&mut buf) {
        Ok(()) => Ok(buf),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            fail("the node closed the session")
        }
        Err(e) => Err(e.into()),
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl Rfb {
    /// Dial a node and complete the 3.8 handshake.
    ///
    /// `addr` comes from the read model's `address` field. It is never taken
    /// from a request body or a page -- see `seat.rs`, which looks the node up
    /// in the state document before it gets here.
    pub fn connect(addr: &str, port: u16) -> Result<Rfb, Error> {
        let target = (addr, port);
        let mut resolved = match target.to_socket_addrs() {
            Ok(it) => it,
            Err(e) => return fail(format!("cannot resolve {}: {}", addr, e)),
        };
        let sa = match resolved.next() {
            Some(sa) => sa,
            None => return fail(format!("cannot resolve {}", addr)),
        };
        let mut sock = match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
            Ok(s) => s,
            Err(e) => {
                return fail(format!(
                    "no VNC on {}:{} -- {}. Is x11vnc running on that node?",
                    addr, port, e
                ))
            }
        };
        sock.set_read_timeout(Some(IO_TIMEOUT))?;
        sock.set_write_timeout(Some(IO_TIMEOUT))?;
        // A seat is latency-bound, not throughput-bound: a 4-byte pointer
        // event should leave now, not when the buffer is comfortable.
        let _ = sock.set_nodelay(true);

        // ProtocolVersion. The server speaks first.
        let greeting = read_n(&mut sock, 12)?;
        let version = String::from_utf8_lossy(&greeting).to_string();
        if !version.starts_with("RFB ") {
            return fail(format!("not a VNC server: it opened with {:?}", version.trim_end()));
        }
        // We answer 3.8 and only 3.8. A server older than that takes a
        // different, wordier handshake, and x11vnc and wayvnc both speak 3.8.
        if version.as_str() < "RFB 003.008" {
            return fail(format!(
                "this speaks {} and the seat needs RFB 003.008 or newer",
                version.trim_end()
            ));
        }
        sock.write_all(b"RFB 003.008\n")?;

        Self::security(&mut sock)?;

        // ClientInit. 1 means "shared": do not disconnect anyone else who is
        // already looking. On a gallery machine the other viewer may be a
        // visitor standing in front of it.
        sock.write_all(&[1u8])?;

        // ServerInit: geometry, pixel format, then a length-prefixed name.
        let head = read_n(&mut sock, 24)?;
        let w = be16(&head[0..2]) as usize;
        let h = be16(&head[2..4]) as usize;
        if w == 0 || h == 0 {
            return fail(format!("the node reports a {}x{} screen", w, h));
        }
        if w * h * 3 > MAX_FRAMEBUFFER {
            return fail(format!("the node reports a {}x{} screen, which is too large", w, h));
        }
        let name_len = be32(&head[20..24]) as usize;
        if name_len > 4096 {
            return fail("the node's desktop name is implausibly long");
        }
        let name = String::from_utf8_lossy(&read_n(&mut sock, name_len)?).to_string();

        let tx = Arc::new(Mutex::new(sock.try_clone()?));
        let mut rfb = Rfb {
            sock,
            tx,
            info: ServerInfo { name, w, h },
            screen: Screen::new(w, h),
            painted: false,
        };
        rfb.set_pixel_format()?;
        rfb.set_encodings()?;
        Ok(rfb)
    }

    /// A handle for sending input, safe to move to another thread.
    pub fn input(&self) -> Input {
        Input { tx: Arc::clone(&self.tx) }
    }

    /// Choose a security type and satisfy it.
    ///
    /// ONLY `None` IS SUPPORTED, AND THAT IS A DECISION WITH A BOUNDARY ROUND
    /// IT. VNC's own authentication is a DES challenge-response with a 56-bit
    /// key and an 8-character password truncation -- it is not security, it is
    /// a speed bump, and implementing DES here would mean writing a cipher in
    /// a program whose README boasts about having no cryptography to get
    /// wrong. Invariant 6 puts the fleet on a LAN that never reaches the
    /// internet, and the node-side gate is the forced command and the
    /// certificate, not a VNC password. So: the seat requires x11vnc to be
    /// started with `-nopw`, and refuses in a sentence that says so rather
    /// than half-implementing a cipher.
    fn security(sock: &mut TcpStream) -> Result<(), Error> {
        let count = read_n(sock, 1)?[0];
        if count == 0 {
            // A zero count means refusal, and the reason follows as a string.
            let len = be32(&read_n(sock, 4)?) as usize;
            let why = if len <= 4096 {
                String::from_utf8_lossy(&read_n(sock, len.min(4096))?).to_string()
            } else {
                "no reason given".to_string()
            };
            return fail(format!("the node refused the session: {}", why));
        }
        let offered = read_n(sock, count as usize)?;
        if !offered.contains(&SEC_NONE) {
            let names: Vec<String> = offered
                .iter()
                .map(|t| match *t {
                    SEC_VNC_AUTH => "VNC password".to_string(),
                    other => format!("type {}", other),
                })
                .collect();
            return fail(format!(
                "this VNC server wants {} and the seat only speaks 'none'. \
                 Start x11vnc with -nopw; the fleet's gate is the certificate, not a VNC password.",
                names.join(" or ")
            ));
        }
        sock.write_all(&[SEC_NONE])?;

        // SecurityResult. 3.8 sends this even for `None`, which is exactly why
        // the seat does not try to speak to anything older.
        let result = be32(&read_n(sock, 4)?);
        if result != 0 {
            let len = be32(&read_n(sock, 4)?) as usize;
            let why = String::from_utf8_lossy(&read_n(sock, len.min(4096))?).to_string();
            return fail(format!("the node refused the session: {}", why));
        }
        Ok(())
    }

    /// Ask for 24-bit true colour in the byte order `Screen` already uses.
    ///
    /// The server converts, which costs it something -- but the alternative is
    /// a converter here for every format a server might prefer, and the whole
    /// point of choosing is to have exactly one path through the decoder.
    fn set_pixel_format(&mut self) -> Result<(), Error> {
        let msg: [u8; 20] = [
            0, // SetPixelFormat
            0, 0, 0, // padding
            32,   // bits per pixel -- 4-byte pixels, so rows never straddle
            24,   // depth
            1,    // big endian: the wire is big endian everywhere else too
            1,    // true colour
            0, 255, // red max
            0, 255, // green max
            0, 255, // blue max
            16, // red shift
            8,  // green shift
            0,  // blue shift
            0, 0, 0, // padding
        ];
        send(&self.tx, &msg)
    }

    /// Raw and CopyRect, plus DesktopSize so a resolution change is a message
    /// rather than a stream of rectangles that no longer fit.
    fn set_encodings(&mut self) -> Result<(), Error> {
        const RAW: i32 = 0;
        const COPY_RECT: i32 = 1;
        const DESKTOP_SIZE: i32 = -223;
        let list = [RAW, COPY_RECT, DESKTOP_SIZE];

        let mut msg = Vec::with_capacity(4 + list.len() * 4);
        msg.push(2u8); // SetEncodings
        msg.push(0); // padding
        msg.extend_from_slice(&(list.len() as u16).to_be_bytes());
        for e in list {
            msg.extend_from_slice(&e.to_be_bytes());
        }
        send(&self.tx, &msg)
    }

    /// Ask for what has changed. `incremental` false demands the whole screen,
    /// which is what the first frame and a resize both need.
    pub fn request_update(&mut self, incremental: bool) -> Result<(), Error> {
        let mut msg = Vec::with_capacity(10);
        msg.push(3u8); // FramebufferUpdateRequest
        msg.push(if incremental { 1 } else { 0 });
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&(self.info.w as u16).to_be_bytes());
        msg.extend_from_slice(&(self.info.h as u16).to_be_bytes());
        send(&self.tx, &msg)
    }

    /// Read one server message and apply it.
    ///
    /// Returns true if the screen changed, so the seat can skip re-encoding a
    /// frame nobody would see a difference in -- which on a gallery node
    /// showing a still scene is most of them.
    pub fn pump(&mut self) -> Result<bool, Error> {
        let kind = read_n(&mut self.sock, 1)?[0];
        match kind {
            0 => self.framebuffer_update(),
            // SetColourMapEntries. We asked for true colour, so this should
            // never arrive; read it off the wire rather than desynchronising.
            1 => {
                let head = read_n(&mut self.sock, 5)?;
                let count = be16(&head[3..5]) as usize;
                read_n(&mut self.sock, count * 6)?;
                Ok(false)
            }
            // Bell. A gallery screen ringing at an operator is noise.
            2 => Ok(false),
            // ServerCutText: the node's clipboard. Exchange's business, not
            // Control's, so it is read and dropped.
            3 => {
                let head = read_n(&mut self.sock, 7)?;
                let len = be32(&head[3..7]) as usize;
                if len > 1024 * 1024 {
                    return fail("the node sent an implausible clipboard");
                }
                read_n(&mut self.sock, len)?;
                Ok(false)
            }
            other => fail(format!("the node sent message type {}, which the seat does not know", other)),
        }
    }

    fn framebuffer_update(&mut self) -> Result<bool, Error> {
        let head = read_n(&mut self.sock, 3)?;
        let count = be16(&head[1..3]);
        if count > MAX_RECTS {
            return fail(format!("the node sent {} rectangles in one update", count));
        }
        let mut changed = false;
        for _ in 0..count {
            let r = read_n(&mut self.sock, 12)?;
            let x = be16(&r[0..2]) as usize;
            let y = be16(&r[2..4]) as usize;
            let w = be16(&r[4..6]) as usize;
            let h = be16(&r[6..8]) as usize;
            let enc = be32(&r[8..12]) as i32;

            match enc {
                0 => {
                    self.raw(x, y, w, h)?;
                    changed = true;
                }
                1 => {
                    self.copy_rect(x, y, w, h)?;
                    changed = true;
                }
                -223 => {
                    // DesktopSize: w and h are the new geometry and there is
                    // no pixel data. Everything we had is now the wrong shape.
                    if w == 0 || h == 0 || w * h * 3 > MAX_FRAMEBUFFER {
                        return fail(format!("the node resized to {}x{}", w, h));
                    }
                    self.info.w = w;
                    self.info.h = h;
                    self.screen.resize(w, h);
                    self.request_update(false)?;
                    changed = true;
                }
                other => {
                    // Desynchronised: the length of an encoding we did not ask
                    // for is unknown, so there is no skipping it.
                    return fail(format!(
                        "the node used encoding {}, which the seat did not ask for",
                        other
                    ));
                }
            }
        }
        if changed {
            self.painted = true;
        }
        Ok(changed)
    }

    /// Raw: `w * h` 4-byte pixels, left to right, top to bottom.
    fn raw(&mut self, x: usize, y: usize, w: usize, h: usize) -> Result<(), Error> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        if x + w > self.screen.w || y + h > self.screen.h {
            return fail("the node sent a rectangle that falls outside its own screen");
        }
        // One row at a time: a full-screen rectangle is megabytes and reading
        // it into a second buffer whole would double the seat's footprint on
        // a board that has 512 MB and an exhibit to run.
        let mut row = vec![0u8; w * 4];
        for dy in 0..h {
            self.sock.read_exact(&mut row)?;
            for dx in 0..w {
                let p = dx * 4;
                // Big endian, 32bpp, shifts 16/8/0: byte 0 is padding.
                self.screen.put(x + dx, y + dy, row[p + 1], row[p + 2], row[p + 3]);
            }
        }
        Ok(())
    }

    /// CopyRect: this area is already on screen somewhere else. A scrolling
    /// window is mostly this, which is why it is worth the fifteen lines.
    fn copy_rect(&mut self, x: usize, y: usize, w: usize, h: usize) -> Result<(), Error> {
        let src = read_n(&mut self.sock, 4)?;
        let sx = be16(&src[0..2]) as usize;
        let sy = be16(&src[2..4]) as usize;
        if x + w > self.screen.w
            || y + h > self.screen.h
            || sx + w > self.screen.w
            || sy + h > self.screen.h
        {
            return fail("the node asked to copy a rectangle that is off-screen");
        }
        // Copy through a scratch buffer: source and destination overlap on
        // every scroll, and doing it in place would smear.
        let mut tmp = vec![0u8; w * h * 3];
        for dy in 0..h {
            let s = ((sy + dy) * self.screen.w + sx) * 3;
            tmp[dy * w * 3..(dy + 1) * w * 3].copy_from_slice(&self.screen.px[s..s + w * 3]);
        }
        for dy in 0..h {
            let d = ((y + dy) * self.screen.w + x) * 3;
            self.screen.px[d..d + w * 3].copy_from_slice(&tmp[dy * w * 3..(dy + 1) * w * 3]);
        }
        Ok(())
    }

}

// ------------------------------------------------------------------ input ---

impl Input {
    /// A key, as an X11 keysym. Down then up is a press.
    pub fn key(&self, keysym: u32, down: bool) -> Result<(), Error> {
        let mut msg = Vec::with_capacity(8);
        msg.push(4u8); // KeyEvent
        msg.push(if down { 1 } else { 0 });
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&keysym.to_be_bytes());
        send(&self.tx, &msg)
    }

    pub fn tap(&self, keysym: u32) -> Result<(), Error> {
        self.key(keysym, true)?;
        self.key(keysym, false)
    }

    /// A key with modifiers held around it, so Ctrl-C reaches the node as
    /// Ctrl-C rather than as the byte 0x03, which is what the terminal hands
    /// us and which means nothing to an X server.
    pub fn chord(&self, mods: &[u32], keysym: u32) -> Result<(), Error> {
        for m in mods {
            self.key(*m, true)?;
        }
        let r = self.tap(keysym);
        // Release the modifiers even if the key itself failed: a half-sent
        // chord that leaves Ctrl down is the exact failure `release_all`
        // exists to prevent.
        for m in mods.iter().rev() {
            let _ = self.key(*m, false);
        }
        r
    }

    /// The pointer. `buttons` is a bitmask: 1 left, 2 middle, 4 right, and
    /// 8/16 are the wheel, which VNC models as buttons 4 and 5.
    pub fn pointer(&self, x: usize, y: usize, buttons: u8) -> Result<(), Error> {
        let mut msg = Vec::with_capacity(6);
        msg.push(5u8); // PointerEvent
        msg.push(buttons);
        msg.extend_from_slice(&(x.min(u16::MAX as usize) as u16).to_be_bytes());
        msg.extend_from_slice(&(y.min(u16::MAX as usize) as u16).to_be_bytes());
        send(&self.tx, &msg)
    }

    /// Let go of everything.
    ///
    /// THE ONE INVARIANT A SEAT MUST NOT BREAK is that the operator cannot get
    /// stuck inside a machine -- `wall.html` says so in its own words and it is
    /// just as true here. A seat that exits holding Ctrl down has left a
    /// gallery node unusable for the next visitor, so leaving Control runs
    /// this, and so does dropping the connection.
    pub fn release_all(&self) {
        // Ctrl, Alt, Shift (both sides), Super, and the buttons.
        for sym in [
            0xffe3u32, 0xffe4, // Control_L, Control_R
            0xffe9, 0xffea, // Alt_L, Alt_R
            0xffe1, 0xffe2, // Shift_L, Shift_R
            0xffeb, 0xffec, // Super_L, Super_R
        ] {
            let _ = self.key(sym, false);
        }
        let _ = self.pointer(0, 0, 0);
    }
}

impl Drop for Rfb {
    fn drop(&mut self) {
        self.input().release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_is_three_bytes_a_pixel() {
        let s = Screen::new(4, 2);
        assert_eq!(s.px.len(), 24);
    }

    #[test]
    fn put_and_get_agree() {
        let mut s = Screen::new(3, 3);
        s.put(2, 1, 10, 20, 30);
        assert_eq!(s.get(2, 1), (10, 20, 30));
        assert_eq!(s.get(0, 0), (0, 0, 0));
    }

    #[test]
    fn resizing_clears() {
        let mut s = Screen::new(2, 2);
        s.put(0, 0, 9, 9, 9);
        s.resize(4, 4);
        assert_eq!(s.w, 4);
        assert_eq!(s.px.len(), 48);
        assert_eq!(s.get(0, 0), (0, 0, 0));
    }

    #[test]
    fn big_endian_readers() {
        assert_eq!(be16(&[0x12, 0x34]), 0x1234);
        assert_eq!(be32(&[0x00, 0x00, 0x01, 0x00]), 256);
    }

    // The pixel format is the decoder's only assumption, so it is asserted
    // here rather than left to be discovered against a live server.
    #[test]
    fn the_pixel_format_matches_what_raw_decodes() {
        // bpp 32, depth 24, big endian, true colour, shifts 16/8/0 means the
        // four bytes on the wire are [pad, R, G, B] -- which is exactly what
        // `raw` indexes as row[p+1], row[p+2], row[p+3].
        let msg: [u8; 20] = [
            0, 0, 0, 0, 32, 24, 1, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0,
        ];
        assert_eq!(msg[4], 32, "bits per pixel");
        assert_eq!(msg[6], 1, "big endian");
        assert_eq!(msg[14], 16, "red shift puts R in byte 1");
        assert_eq!(msg[15], 8, "green shift puts G in byte 2");
        assert_eq!(msg[16], 0, "blue shift puts B in byte 3");
    }
}

/// An RFB 3.8 server, just enough of one to prove the client against.
///
/// THIS EXISTS BECAUSE THE PROTOCOL CODE ABOVE IS THE KIND THAT LOOKS RIGHT
/// AND IS OFF BY ONE BYTE. Unit tests on `Screen` prove arithmetic; they do
/// not prove that the handshake is in the right order or that a Raw rectangle
/// is indexed correctly, and the only thing that proves those is a socket.
/// Nothing here is a mock of the client -- it is the other side of the wire,
/// written from the RFB specification, and the client does not know it is a
/// test.
#[cfg(test)]
pub mod fake {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    pub struct Fake {
        pub port: u16,
        pub w: usize,
        pub h: usize,
    }

    /// Serve exactly one client, then stop.
    ///
    /// `paint` fills the framebuffer the server will send: `(x, y) -> rgb`.
    pub fn serve(
        w: usize,
        h: usize,
        security: &[u8],
        paint: fn(usize, usize) -> (u8, u8, u8),
    ) -> (Fake, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let sec = security.to_vec();

        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            // ProtocolVersion
            s.write_all(b"RFB 003.008\n").unwrap();
            let mut v = [0u8; 12];
            s.read_exact(&mut v).unwrap();

            // Security
            s.write_all(&[sec.len() as u8]).unwrap();
            s.write_all(&sec).unwrap();
            let mut chosen = [0u8; 1];
            if s.read_exact(&mut chosen).is_err() {
                return Vec::new();
            }
            if !sec.contains(&1) {
                return Vec::new();
            }
            s.write_all(&0u32.to_be_bytes()).unwrap(); // SecurityResult: ok

            // ClientInit
            let mut shared = [0u8; 1];
            s.read_exact(&mut shared).unwrap();
            assert_eq!(shared[0], 1, "the seat must not evict another viewer");

            // ServerInit
            let mut init = Vec::new();
            init.extend_from_slice(&(w as u16).to_be_bytes());
            init.extend_from_slice(&(h as u16).to_be_bytes());
            // A pixel format the client will immediately override.
            init.extend_from_slice(&[32, 24, 1, 1]);
            init.extend_from_slice(&255u16.to_be_bytes());
            init.extend_from_slice(&255u16.to_be_bytes());
            init.extend_from_slice(&255u16.to_be_bytes());
            init.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
            let name = b"museum-01:0";
            init.extend_from_slice(&(name.len() as u32).to_be_bytes());
            init.extend_from_slice(name);
            s.write_all(&init).unwrap();

            // Everything the client says from here on is recorded and handed
            // back, so a test can assert on the bytes it put on the wire.
            let mut said = Vec::new();
            let mut sent_frame = false;
            loop {
                let mut kind = [0u8; 1];
                if s.read_exact(&mut kind).is_err() {
                    return said;
                }
                let extra = match kind[0] {
                    0 => 19, // SetPixelFormat
                    2 => {
                        let mut head = [0u8; 3];
                        if s.read_exact(&mut head).is_err() {
                            return said;
                        }
                        said.push(2);
                        said.extend_from_slice(&head);
                        let n = u16::from_be_bytes([head[1], head[2]]) as usize;
                        let mut body = vec![0u8; n * 4];
                        if s.read_exact(&mut body).is_err() {
                            return said;
                        }
                        said.extend_from_slice(&body);
                        continue;
                    }
                    3 => 9,  // FramebufferUpdateRequest
                    4 => 7,  // KeyEvent
                    5 => 5,  // PointerEvent
                    _ => return said,
                };
                let mut body = vec![0u8; extra];
                if s.read_exact(&mut body).is_err() {
                    return said;
                }
                said.push(kind[0]);
                said.extend_from_slice(&body);

                if kind[0] == 3 && !sent_frame {
                    sent_frame = true;
                    send_raw(&mut s, w, h, paint);
                }
            }
        });

        (Fake { port, w, h }, handle)
    }

    /// One FramebufferUpdate carrying one full-screen Raw rectangle.
    fn send_raw(s: &mut TcpStream, w: usize, h: usize, paint: fn(usize, usize) -> (u8, u8, u8)) {
        let mut msg = vec![0u8, 0]; // FramebufferUpdate, padding
        msg.extend_from_slice(&1u16.to_be_bytes()); // one rectangle
        msg.extend_from_slice(&0u16.to_be_bytes()); // x
        msg.extend_from_slice(&0u16.to_be_bytes()); // y
        msg.extend_from_slice(&(w as u16).to_be_bytes());
        msg.extend_from_slice(&(h as u16).to_be_bytes());
        msg.extend_from_slice(&0i32.to_be_bytes()); // Raw
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = paint(x, y);
                // The format the client asked for: big endian, 32bpp,
                // shifts 16/8/0 -- so [pad, R, G, B].
                msg.extend_from_slice(&[0, r, g, b]);
            }
        }
        let _ = s.write_all(&msg);
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn gradient(x: usize, y: usize) -> (u8, u8, u8) {
        (x as u8, y as u8, 0x40)
    }

    #[test]
    fn it_handshakes_and_decodes_a_real_raw_frame() {
        let (fake, handle) = fake::serve(16, 8, &[1], gradient);
        let mut rfb = Rfb::connect("127.0.0.1", fake.port).expect("connect");
        assert_eq!(rfb.info.w, 16);
        assert_eq!(rfb.info.h, 8);
        assert_eq!(rfb.info.name, "museum-01:0");

        rfb.request_update(false).expect("request");
        let changed = rfb.pump().expect("pump");
        assert!(changed, "a full-screen Raw rectangle must count as a change");
        assert!(rfb.painted);

        // The decode is correct or it is off by a byte, and this is the check.
        for (x, y) in [(0usize, 0usize), (15, 7), (3, 5), (8, 2)] {
            assert_eq!(rfb.screen.get(x, y), gradient(x, y), "pixel {},{}", x, y);
        }

        drop(rfb);
        let said = handle.join().unwrap();
        assert!(!said.is_empty());
    }

    #[test]
    fn it_asks_for_the_encodings_it_can_actually_decode() {
        let (fake, handle) = fake::serve(4, 4, &[1], gradient);
        let rfb = Rfb::connect("127.0.0.1", fake.port).expect("connect");
        drop(rfb);
        let said = handle.join().unwrap();

        // Find the SetEncodings message and read the list back.
        let i = said.iter().position(|&b| b == 2).expect("no SetEncodings");
        let n = u16::from_be_bytes([said[i + 2], said[i + 3]]) as usize;
        let mut encodings = Vec::new();
        for k in 0..n {
            let p = i + 4 + k * 4;
            encodings.push(i32::from_be_bytes([
                said[p], said[p + 1], said[p + 2], said[p + 3],
            ]));
        }
        assert!(encodings.contains(&0), "Raw must be asked for");
        assert!(encodings.contains(&1), "CopyRect must be asked for");
        assert!(encodings.contains(&-223), "DesktopSize must be asked for");
        // The point of the list is that it is short: asking for an encoding
        // the decoder cannot read desynchronises the session permanently.
        assert_eq!(encodings.len(), 3, "asked for {:?}", encodings);
    }

    #[test]
    fn input_lands_on_the_wire_as_rfb_says_it_should() {
        let (fake, handle) = fake::serve(4, 4, &[1], gradient);
        let rfb = Rfb::connect("127.0.0.1", fake.port).expect("connect");
        let input = rfb.input();
        input.key(0x0061, true).unwrap(); // 'a' down
        input.pointer(300, 200, 1).unwrap();
        drop(rfb);
        // AN `Input` HOLDS THE SESSION OPEN. It carries an Arc over the write
        // half, so the socket outlives the `Rfb` for exactly as long as any
        // handle does -- which is what makes it safe to give one to another
        // thread, and which means the seat must let go of its own before it
        // can expect the node to see the session end.
        drop(input);
        let said = handle.join().unwrap();

        let key = said
            .windows(8)
            .find(|w| w[0] == 4 && w[1] == 1)
            .expect("no KeyEvent on the wire");
        assert_eq!(u32::from_be_bytes([key[4], key[5], key[6], key[7]]), 0x61);

        let ptr = said
            .windows(6)
            .find(|w| w[0] == 5 && w[1] == 1)
            .expect("no PointerEvent on the wire");
        assert_eq!(u16::from_be_bytes([ptr[2], ptr[3]]), 300);
        assert_eq!(u16::from_be_bytes([ptr[4], ptr[5]]), 200);
    }

    #[test]
    fn dropping_the_session_releases_every_modifier() {
        let (fake, handle) = fake::serve(4, 4, &[1], gradient);
        let rfb = Rfb::connect("127.0.0.1", fake.port).expect("connect");
        drop(rfb);
        let said = handle.join().unwrap();

        // Control_L must have gone up. An operator who closes the seat with a
        // modifier stuck has left a gallery node unusable.
        let found = said.windows(8).any(|w| {
            w[0] == 4 && w[1] == 0 && u32::from_be_bytes([w[4], w[5], w[6], w[7]]) == 0xffe3
        });
        assert!(found, "Control_L was never released");
    }

    #[test]
    fn a_server_that_wants_a_password_is_refused_in_words() {
        // Security type 2 only: VNC authentication, which the seat does not do.
        let (fake, handle) = fake::serve(4, 4, &[2], gradient);
        let e = match Rfb::connect("127.0.0.1", fake.port) {
            Err(e) => e,
            Ok(_) => panic!("a password-only server must be refused"),
        };
        assert!(e.0.contains("VNC password"), "{}", e.0);
        assert!(e.0.contains("-nopw"), "the error must say how to fix it: {}", e.0);
        let _ = handle.join();
    }

    #[test]
    fn nothing_listening_says_so_plainly() {
        // Port 1 on loopback: nothing is there, and the sentence should name
        // x11vnc rather than reciting an errno.
        let e = match Rfb::connect("127.0.0.1", 1) {
            Err(e) => e,
            Ok(_) => panic!("something answered on port 1"),
        };
        assert!(e.0.contains("x11vnc"), "{}", e.0);
    }
}
