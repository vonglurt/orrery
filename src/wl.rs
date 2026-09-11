//! Wayland, by hand -- enough of it to own a window.
//!
//! There is no `wayland-scanner` here and no generated code. Wayland's
//! interfaces are defined in XML and normally turned into a client library by
//! a code generator; what a client actually needs is the wire format plus the
//! opcodes of the dozen interfaces it touches, and that is small enough to
//! write, the same way `http.rs` and `rfb.rs` are written.
//!
//! WHAT THE WIRE IS. Every message is an 8-byte header -- the object id, then
//! one word holding the byte length in the high half and the opcode in the low
//! half -- followed by the arguments. Integers are one word. Strings are a
//! length (counting a trailing NUL) then the bytes then the NUL, padded out to
//! a word. Arrays are the same without the NUL. Descriptors are NOT in the
//! byte stream at all: they travel beside it as ancillary data, which is why
//! `sys.rs` exists.
//!
//! EVERYTHING IS NATIVE BYTE ORDER, which is the one place this differs from
//! every other protocol in this program. RFB and HTTP are big-endian on the
//! wire; Wayland is whatever the machine is, because the compositor is always
//! on the same machine. `to_ne_bytes` throughout, deliberately.
//!
//! OBJECT IDS ARE OURS TO ALLOCATE. Ids from 1 are the client's; the display
//! is always 1, and every object this creates takes the next number up. The
//! compositor never invents an id in this range, so there is no negotiation
//! and no table to keep in sync -- just a counter.
//!
//! Phase 1 scope: a window that opens, paints, resizes and closes. Input lives
//! on `wl_seat`, and it arrives with the seat's own work rather than here.

use std::collections::VecDeque;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::sys::{self, Mapping};

// ------------------------------------------------------------------ errors ---

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

// ------------------------------------------------------------------- wire ---

/// The id of `wl_display`, which is fixed by the protocol.
pub const DISPLAY: u32 = 1;

/// A message being built. The length is not known until the arguments are on,
/// so the header word is written last, over a hole left for it.
pub struct Msg {
    buf: Vec<u8>,
    fds: Vec<RawFd>,
}

impl Msg {
    pub fn new(object: u32, opcode: u16) -> Msg {
        let mut buf = Vec::with_capacity(32);
        buf.extend_from_slice(&object.to_ne_bytes());
        buf.extend_from_slice(&((opcode as u32) << 16).to_ne_bytes());
        Msg { buf, fds: Vec::new() }
    }

    pub fn uint(mut self, v: u32) -> Msg {
        self.buf.extend_from_slice(&v.to_ne_bytes());
        self
    }

    pub fn int(self, v: i32) -> Msg {
        self.uint(v as u32)
    }

    pub fn object(self, id: u32) -> Msg {
        self.uint(id)
    }

    pub fn new_id(self, id: u32) -> Msg {
        self.uint(id)
    }

    /// A string: length including the NUL, the bytes, the NUL, then padding to
    /// the next word.
    ///
    /// AN EMPTY STRING IS NOT A NULL ONE, and the difference is four bytes on
    /// the wire. `""` has length 1 -- the NUL is part of what is counted -- and
    /// occupies eight bytes once padded. Only a *null* string is length 0 with
    /// no body, and nothing here sends one, so there is no constructor for it.
    pub fn string(mut self, s: &str) -> Msg {
        let bytes = s.as_bytes();
        self.buf.extend_from_slice(&((bytes.len() + 1) as u32).to_ne_bytes());
        self.buf.extend_from_slice(bytes);
        self.buf.push(0);
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
        self
    }

    /// `wl_registry.bind` takes the "generic" new_id, which unlike every other
    /// new_id carries the interface name and version with it -- the registry
    /// has no other way to know what it is making.
    pub fn bind_id(self, interface: &str, version: u32, id: u32) -> Msg {
        self.string(interface).uint(version).new_id(id)
    }

    /// A descriptor. It occupies no space in the byte stream.
    pub fn fd(mut self, fd: RawFd) -> Msg {
        self.fds.push(fd);
        self
    }

    /// Stamp the length into the header now that it is known.
    fn finish(mut self) -> (Vec<u8>, Vec<RawFd>) {
        let len = self.buf.len() as u32;
        let word = u32::from_ne_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        let opcode = word >> 16;
        let header = (len << 16) | opcode;
        self.buf[4..8].copy_from_slice(&header.to_ne_bytes());
        (self.buf, self.fds)
    }
}

/// One message from the compositor, already split from the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub object: u32,
    pub opcode: u16,
    /// The arguments, still packed. The reader below takes them apart.
    pub body: Vec<u8>,
}

impl Event {
    pub fn args(&self) -> Args<'_> {
        Args { body: &self.body, at: 0 }
    }
}

/// A cursor over one event's arguments.
pub struct Args<'a> {
    body: &'a [u8],
    at: usize,
}

impl Args<'_> {
    pub fn uint(&mut self) -> Option<u32> {
        if self.at + 4 > self.body.len() {
            return None;
        }
        let v = u32::from_ne_bytes([
            self.body[self.at],
            self.body[self.at + 1],
            self.body[self.at + 2],
            self.body[self.at + 3],
        ]);
        self.at += 4;
        Some(v)
    }

    pub fn int(&mut self) -> Option<i32> {
        self.uint().map(|v| v as i32)
    }

    pub fn string(&mut self) -> Option<String> {
        let len = self.uint()? as usize;
        if len == 0 {
            return Some(String::new());
        }
        let padded = len.div_ceil(4) * 4;
        if self.at + padded > self.body.len() {
            return None;
        }
        // The length counts the NUL; the string is everything before it.
        let s = String::from_utf8_lossy(&self.body[self.at..self.at + len - 1]).to_string();
        self.at += padded;
        Some(s)
    }

    /// An array argument, returned as its raw bytes. `xdg_toplevel.configure`
    /// carries its states this way.
    pub fn array(&mut self) -> Option<Vec<u8>> {
        let len = self.uint()? as usize;
        let padded = len.div_ceil(4) * 4;
        if self.at + padded > self.body.len() {
            return None;
        }
        let v = self.body[self.at..self.at + len].to_vec();
        self.at += padded;
        Some(v)
    }
}

// ------------------------------------------------------------- connection ---

/// The socket, the id counter, and the descriptors that arrived with events.
pub struct Conn {
    sock: UnixStream,
    next_id: u32,
    inbuf: Vec<u8>,
    events: VecDeque<Event>,
    fds: VecDeque<RawFd>,
}

impl Conn {
    /// Find the compositor the way every Wayland client is expected to.
    ///
    /// `WAYLAND_SOCKET` wins when it is set: a compositor that launches a
    /// client hands it an already-connected descriptor and expects it to be
    /// used rather than a second connection opened behind its back.
    pub fn connect() -> Result<Conn, Error> {
        if let Ok(s) = std::env::var("WAYLAND_SOCKET") {
            if let Ok(fd) = s.parse::<RawFd>() {
                // SAFETY: the compositor promised this descriptor is a
                // connected socket and that it is ours to own.
                let sock = unsafe {
                    <UnixStream as std::os::unix::io::FromRawFd>::from_raw_fd(fd)
                };
                std::env::remove_var("WAYLAND_SOCKET");
                return Conn::wrap(sock);
            }
        }

        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = if display.starts_with('/') {
            PathBuf::from(display)
        } else {
            let dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| {
                Error(
                    "XDG_RUNTIME_DIR is not set, so there is nowhere to look for a \
                     compositor. On Copal that variable is set at login by copal-session."
                        .into(),
                )
            })?;
            PathBuf::from(dir).join(display)
        };

        let sock = UnixStream::connect(&path).map_err(|e| {
            Error(format!(
                "no compositor on {} -- {}. Is this a Wayland session? \
                 /etc/copal/session says which desktop starts, and its default is x11.",
                path.display(),
                e
            ))
        })?;
        Conn::wrap(sock)
    }

    pub fn wrap(sock: UnixStream) -> Result<Conn, Error> {
        Ok(Conn {
            sock,
            // 1 is the display; everything this program makes starts above it.
            next_id: 2,
            inbuf: Vec::with_capacity(4096),
            events: VecDeque::new(),
            fds: VecDeque::new(),
        })
    }

    pub fn allocate(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn send(&mut self, msg: Msg) -> Result<(), Error> {
        let (buf, fds) = msg.finish();
        let mut sent = 0;
        while sent < buf.len() {
            // The descriptors ride on the first byte, so they go with the
            // first write and never with a continuation.
            let with = if sent == 0 { &fds[..] } else { &[][..] };
            let n = sys::send_with_fds(self.sock.as_raw_fd(), &buf[sent..], with)?;
            if n == 0 {
                return fail("the compositor stopped reading");
            }
            sent += n;
        }
        // The descriptors have been handed to the kernel; this process is done
        // with its own copies. Leaving them open leaks one per buffer.
        for fd in fds {
            sys::close_fd(fd);
        }
        Ok(())
    }

    /// Read whatever is there and split out every complete message.
    pub fn fill(&mut self) -> Result<(), Error> {
        let mut buf = [0u8; 4096];
        let mut fds = Vec::new();
        let n = sys::recv_with_fds(self.sock.as_raw_fd(), &mut buf, &mut fds)?;
        if n == 0 {
            return fail("the compositor closed the connection");
        }
        self.inbuf.extend_from_slice(&buf[..n]);
        for fd in fds {
            self.fds.push_back(fd);
        }
        self.split();
        Ok(())
    }

    /// Take apart as many whole messages as the buffer holds.
    ///
    /// A short read in the middle of a message is normal and must leave the
    /// remainder alone -- this is the loop that a stream protocol needs and
    /// that a datagram one does not.
    fn split(&mut self) {
        loop {
            if self.inbuf.len() < 8 {
                return;
            }
            let object = u32::from_ne_bytes([
                self.inbuf[0], self.inbuf[1], self.inbuf[2], self.inbuf[3],
            ]);
            let word = u32::from_ne_bytes([
                self.inbuf[4], self.inbuf[5], self.inbuf[6], self.inbuf[7],
            ]);
            let len = (word >> 16) as usize;
            let opcode = (word & 0xffff) as u16;
            // A length below the header, or not a whole number of words, means
            // the stream is no longer aligned to messages and nothing after it
            // can be trusted.
            if len < 8 || len % 4 != 0 {
                self.inbuf.clear();
                return;
            }
            if self.inbuf.len() < len {
                return;
            }
            let body = self.inbuf[8..len].to_vec();
            self.inbuf.drain(..len);
            self.events.push_back(Event { object, opcode, body });
        }
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Block until at least one event is available.
    pub fn wait(&mut self) -> Result<Event, Error> {
        loop {
            if let Some(e) = self.events.pop_front() {
                return Ok(e);
            }
            self.fill()?;
        }
    }

    /// The next descriptor the compositor sent, in arrival order.
    pub fn take_fd(&mut self) -> Option<RawFd> {
        self.fds.pop_front()
    }

    pub fn set_nonblocking(&self, yes: bool) -> Result<(), Error> {
        self.sock.set_nonblocking(yes)?;
        Ok(())
    }

    pub fn raw_fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    /// Turn a `wl_display.error` event into the sentence it is.
    ///
    /// A protocol error is fatal and the compositor closes the connection
    /// straight after sending it, so reporting it exactly is the difference
    /// between a fixable bug and a client that "just exits".
    pub fn describe_error(e: &Event) -> Option<String> {
        if e.object != DISPLAY || e.opcode != 0 {
            return None;
        }
        let mut a = e.args();
        let object = a.uint()?;
        let code = a.uint()?;
        let message = a.string().unwrap_or_default();
        Some(format!(
            "the compositor refused this client: {} (object {}, code {})",
            message, object, code
        ))
    }
}

// -------------------------------------------------------------- interfaces ---
//
// Opcodes, taken from wayland.xml and xdg-shell.xml. They are positional --
// the nth request in the XML is opcode n -- so they are listed in order and
// named, rather than written as bare numbers at the call sites.

pub mod display {
    pub const SYNC: u16 = 0;
    pub const GET_REGISTRY: u16 = 1;
    pub const EV_ERROR: u16 = 0;
    pub const EV_DELETE_ID: u16 = 1;
}

pub mod registry {
    pub const BIND: u16 = 0;
    pub const EV_GLOBAL: u16 = 0;
    pub const EV_GLOBAL_REMOVE: u16 = 1;
}

pub mod callback {
    pub const EV_DONE: u16 = 0;
}

pub mod compositor {
    pub const CREATE_SURFACE: u16 = 0;
}

pub mod shm {
    pub const CREATE_POOL: u16 = 0;
    pub const EV_FORMAT: u16 = 0;
    /// 32-bit, x:R:G:B, native endian. No alpha, because a window that is
    /// accidentally translucent over a gallery screen is a bug nobody reports.
    pub const XRGB8888: u32 = 1;
}

pub mod shm_pool {
    pub const CREATE_BUFFER: u16 = 0;
    pub const DESTROY: u16 = 1;
    pub const RESIZE: u16 = 2;
}

pub mod buffer {
    pub const DESTROY: u16 = 0;
    pub const EV_RELEASE: u16 = 0;
}

pub mod surface {
    pub const DESTROY: u16 = 0;
    pub const ATTACH: u16 = 1;
    pub const DAMAGE: u16 = 2;
    pub const FRAME: u16 = 3;
    pub const COMMIT: u16 = 6;
}

pub mod wm_base {
    pub const DESTROY: u16 = 0;
    pub const GET_XDG_SURFACE: u16 = 2;
    pub const PONG: u16 = 3;
    pub const EV_PING: u16 = 0;
}

pub mod xdg_surface {
    pub const DESTROY: u16 = 0;
    pub const GET_TOPLEVEL: u16 = 1;
    pub const ACK_CONFIGURE: u16 = 4;
    pub const EV_CONFIGURE: u16 = 0;
}

pub mod toplevel {
    pub const DESTROY: u16 = 0;
    pub const SET_TITLE: u16 = 2;
    pub const SET_APP_ID: u16 = 3;
    pub const EV_CONFIGURE: u16 = 0;
    pub const EV_CLOSE: u16 = 1;
}

// ------------------------------------------------------------------ window ---

/// What the compositor has told us since the last time anyone asked.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    /// A new size was configured and the buffer no longer matches it.
    pub resized: bool,
    /// The user closed the window. Honour it.
    pub closed: bool,
}

/// One toplevel window with a shared-memory buffer behind it.
pub struct Window {
    conn: Conn,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
    shm: u32,
    wm_base: u32,
    pool: u32,
    pool_buffer: u32,
    map: Mapping,
    pool_bytes: usize,
    pub width: usize,
    pub height: usize,
    /// Set between a configure that changed the size and the next `paint`.
    pending_size: Option<(usize, usize)>,
    closed: bool,
    /// True while the compositor holds the buffer. Painting into a buffer the
    /// compositor is still reading is the classic Wayland tear.
    buffer_busy: bool,
}

/// What `bind` found in the registry.
struct Globals {
    compositor: u32,
    shm: u32,
    wm_base: u32,
}

impl Window {
    /// Open a window of `w` by `h` and give it a title.
    pub fn open(title: &str, w: usize, h: usize) -> Result<Window, Error> {
        let conn = Conn::connect()?;
        Window::on(conn, title, w, h)
    }

    /// The same, on a connection someone else made -- which is how the tests
    /// point this at a compositor of their own.
    pub fn on(mut conn: Conn, title: &str, w: usize, h: usize) -> Result<Window, Error> {
        let g = discover(&mut conn)?;

        let surface = conn.allocate();
        conn.send(Msg::new(g.compositor, compositor::CREATE_SURFACE).new_id(surface))?;

        let xdg_surf = conn.allocate();
        conn.send(
            Msg::new(g.wm_base, wm_base::GET_XDG_SURFACE)
                .new_id(xdg_surf)
                .object(surface),
        )?;

        let toplevel = conn.allocate();
        conn.send(Msg::new(xdg_surf, xdg_surface::GET_TOPLEVEL).new_id(toplevel))?;
        conn.send(Msg::new(toplevel, toplevel::SET_TITLE).string(title))?;
        conn.send(Msg::new(toplevel, toplevel::SET_APP_ID).string("org.copal.orrery"))?;

        // A surface must be committed with no buffer before the compositor
        // will send its first configure. Attaching one now is a protocol
        // error, and this is the ordering every xdg-shell client gets wrong
        // once.
        conn.send(Msg::new(surface, surface::COMMIT))?;

        let bytes = w * h * 4;
        // THE DESCRIPTOR IS NOT KEPT. `Mapping` holds the memory and the
        // compositor gets its own reference when the pool is created, so
        // `Conn::send` closes this end and nothing here may close it again --
        // an earlier version stored the number and closed it twice, which on a
        // busy process closes whatever was handed that number next. Here, the
        // Wayland socket.
        let fd = sys::memfd("orrery-wl", bytes)?;
        let map = Mapping::new(fd, bytes)?;

        let pool = conn.allocate();
        conn.send(
            Msg::new(g.shm, shm::CREATE_POOL)
                .new_id(pool)
                .fd(fd)
                .int(bytes as i32),
        )?;

        let buf = conn.allocate();
        conn.send(
            Msg::new(pool, shm_pool::CREATE_BUFFER)
                .new_id(buf)
                .int(0)
                .int(w as i32)
                .int(h as i32)
                .int((w * 4) as i32)
                .uint(shm::XRGB8888),
        )?;

        let mut win = Window {
            conn,
            surface,
            xdg_surface: xdg_surf,
            toplevel,
            shm: g.shm,
            wm_base: g.wm_base,
            pool,
            pool_buffer: buf,
            map,
            pool_bytes: bytes,
            width: w,
            height: h,
            pending_size: None,
            closed: false,
            buffer_busy: false,
        };

        // Wait for the first configure and acknowledge it, or the window never
        // appears.
        win.settle()?;
        Ok(win)
    }

    /// Pump events until the surface has been configured at least once.
    fn settle(&mut self) -> Result<(), Error> {
        for _ in 0..256 {
            let e = self.conn.wait()?;
            if let Some(why) = Conn::describe_error(&e) {
                return fail(why);
            }
            let configured = self.handle(&e)?;
            if configured {
                return Ok(());
            }
        }
        fail("the compositor never configured the window")
    }

    /// Apply one event. Returns true when it was the xdg_surface configure,
    /// which is the handshake everything else waits on.
    fn handle(&mut self, e: &Event) -> Result<bool, Error> {
        if let Some(why) = Conn::describe_error(e) {
            return fail(why);
        }
        if e.object == self.xdg_surface && e.opcode == xdg_surface::EV_CONFIGURE {
            let serial = e.args().uint().unwrap_or(0);
            self.conn
                .send(Msg::new(self.xdg_surface, xdg_surface::ACK_CONFIGURE).uint(serial))?;
            return Ok(true);
        }
        if e.object == self.toplevel && e.opcode == toplevel::EV_CONFIGURE {
            let mut a = e.args();
            let w = a.int().unwrap_or(0);
            let h = a.int().unwrap_or(0);
            // Zero means "you choose", which is what arrives on the first
            // configure of a window nobody has resized.
            if w > 0 && h > 0 && (w as usize, h as usize) != (self.width, self.height) {
                self.pending_size = Some((w as usize, h as usize));
            }
            return Ok(false);
        }
        if e.object == self.toplevel && e.opcode == toplevel::EV_CLOSE {
            self.closed = true;
            return Ok(false);
        }
        if e.object == self.pool_buffer && e.opcode == buffer::EV_RELEASE {
            self.buffer_busy = false;
            return Ok(false);
        }
        // xdg_wm_base.ping must be ponged or the compositor decides this
        // client has hung and kills it.
        //
        // AN OPCODE ALONE NEVER IDENTIFIES AN EVENT, and this is the line that
        // taught it. `ping` is opcode 0 on xdg_wm_base -- and so is `format`
        // on wl_shm, `enter` on wl_surface and `release` on wl_buffer. Matching
        // the opcode without the object sent a pong to wl_shm, which answered
        // "invalid method 3, object wl_shm#5" and closed the connection. Every
        // arm in this function names its object for that reason.
        if e.object == self.wm_base && e.opcode == wm_base::EV_PING {
            let serial = e.args().uint().unwrap_or(0);
            self.conn.send(Msg::new(self.wm_base, wm_base::PONG).uint(serial))?;
            return Ok(false);
        }
        Ok(false)
    }

    /// Drain whatever the compositor has said, without blocking.
    pub fn poll(&mut self) -> Result<Changes, Error> {
        let mut changes = Changes::default();
        self.conn.set_nonblocking(true)?;
        let r = self.conn.fill();
        self.conn.set_nonblocking(false)?;
        match r {
            Ok(()) => {}
            Err(Error(ref m)) if m.contains("Resource temporarily unavailable") => {}
            Err(e) => return Err(e),
        }
        while let Some(e) = self.conn.next_event() {
            self.handle(&e)?;
        }
        if self.pending_size.is_some() {
            changes.resized = true;
        }
        changes.closed = self.closed;
        Ok(changes)
    }

    /// The pixels, as `width * height` words of `0x00RRGGBB`.
    pub fn pixels(&mut self) -> &mut [u8] {
        self.map.as_mut()
    }

    /// Fill the whole window with one colour. Phase 1's entire drawing API;
    /// `draw.rs` takes over from here in phase 2.
    pub fn fill(&mut self, r: u8, g: u8, b: u8) {
        let px = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        let word = px.to_ne_bytes();
        let map = self.map.as_mut();
        for chunk in map.chunks_exact_mut(4) {
            chunk.copy_from_slice(&word);
        }
    }

    /// Adopt a size the compositor asked for, reallocating the pool.
    ///
    /// The pool only ever grows. `wl_shm_pool.resize` cannot shrink one, and a
    /// window that is made small and then large again would otherwise need a
    /// new pool every time.
    pub fn apply_resize(&mut self) -> Result<(), Error> {
        let (w, h) = match self.pending_size.take() {
            Some(s) => s,
            None => return Ok(()),
        };
        let need = w * h * 4;

        if need > self.pool_bytes {
            let fd = sys::memfd("orrery-wl", need)?;
            let map = Mapping::new(fd, need)?;
            let pool = self.conn.allocate();
            self.conn.send(
                Msg::new(self.shm, shm::CREATE_POOL)
                    .new_id(pool)
                    .fd(fd)
                    .int(need as i32),
            )?;
            self.conn.send(Msg::new(self.pool, shm_pool::DESTROY))?;
            self.pool = pool;
            self.map = map;
            self.pool_bytes = need;
        }

        self.conn.send(Msg::new(self.pool_buffer, buffer::DESTROY))?;
        let buf = self.conn.allocate();
        self.conn.send(
            Msg::new(self.pool, shm_pool::CREATE_BUFFER)
                .new_id(buf)
                .int(0)
                .int(w as i32)
                .int(h as i32)
                .int((w * 4) as i32)
                .uint(shm::XRGB8888),
        )?;
        self.pool_buffer = buf;
        self.width = w;
        self.height = h;
        self.buffer_busy = false;
        Ok(())
    }

    /// Hand the current buffer to the compositor.
    pub fn present(&mut self) -> Result<(), Error> {
        self.conn.send(
            Msg::new(self.surface, surface::ATTACH)
                .object(self.pool_buffer)
                .int(0)
                .int(0),
        )?;
        // i32::MAX rather than the real size: "everything changed", which is
        // both true after a full repaint and cheaper than tracking rectangles
        // for a window that redraws at six frames a second.
        self.conn.send(
            Msg::new(self.surface, surface::DAMAGE)
                .int(0)
                .int(0)
                .int(i32::MAX)
                .int(i32::MAX),
        )?;
        self.conn.send(Msg::new(self.surface, surface::COMMIT))?;
        self.buffer_busy = true;
        Ok(())
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    /// True while the compositor still holds the buffer.
    ///
    /// This is the only honest proof a client has that its pixels were taken:
    /// `wl_buffer.release` is sent when the compositor is finished reading the
    /// buffer for compositing, so seeing it go false after a `present` means
    /// the frame was actually consumed, not merely posted.
    pub fn buffer_busy(&self) -> bool {
        self.buffer_busy
    }

    pub fn conn_mut(&mut self) -> &mut Conn {
        &mut self.conn
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Politeness the compositor does not require -- it cleans up when the
        // socket closes -- but a client that tears its objects down in order
        // is one whose protocol errors are its own.
        let _ = self.conn.send(Msg::new(self.pool_buffer, buffer::DESTROY));
        let _ = self.conn.send(Msg::new(self.pool, shm_pool::DESTROY));
        let _ = self.conn.send(Msg::new(self.toplevel, toplevel::DESTROY));
        let _ = self.conn.send(Msg::new(self.xdg_surface, xdg_surface::DESTROY));
        let _ = self.conn.send(Msg::new(self.surface, surface::DESTROY));
    }
}

/// Ask the registry what this compositor has, and bind the three we need.
fn discover(conn: &mut Conn) -> Result<Globals, Error> {
    let reg = conn.allocate();
    conn.send(Msg::new(DISPLAY, display::GET_REGISTRY).new_id(reg))?;

    // A sync gives a definite end to the burst of globals: the compositor
    // answers it only after everything already queued has been sent, so a
    // `done` here means the advertisement is complete. Without it there is no
    // way to know whether a missing global is absent or merely late.
    let sync = conn.allocate();
    conn.send(Msg::new(DISPLAY, display::SYNC).new_id(sync))?;

    let mut compositor_id = 0;
    let mut shm_id = 0;
    let mut wm_base_id = 0;
    let mut seen: Vec<String> = Vec::new();

    loop {
        let e = conn.wait()?;
        if let Some(why) = Conn::describe_error(&e) {
            return fail(why);
        }
        if e.object == sync && e.opcode == callback::EV_DONE {
            break;
        }
        if e.object != reg || e.opcode != registry::EV_GLOBAL {
            continue;
        }
        let mut a = e.args();
        let name = match a.uint() {
            Some(n) => n,
            None => continue,
        };
        let interface = match a.string() {
            Some(s) => s,
            None => continue,
        };
        let version = a.uint().unwrap_or(1);
        seen.push(interface.clone());

        // Bind the lowest version that has what is used here. Asking for more
        // than is needed is how a client stops working on an older compositor
        // for no reason it can point at.
        let (slot, want) = match interface.as_str() {
            "wl_compositor" => (&mut compositor_id, 1u32),
            "wl_shm" => (&mut shm_id, 1),
            "xdg_wm_base" => (&mut wm_base_id, 1),
            _ => continue,
        };
        if *slot != 0 {
            continue;
        }
        let id = conn.allocate();
        let v = want.min(version);
        conn.send(Msg::new(reg, registry::BIND).uint(name).bind_id(&interface, v, id))?;
        *slot = id;
    }

    let missing: Vec<&str> = [
        ("wl_compositor", compositor_id),
        ("wl_shm", shm_id),
        ("xdg_wm_base", wm_base_id),
    ]
    .iter()
    .filter(|(_, id)| *id == 0)
    .map(|(n, _)| *n)
    .collect();

    if !missing.is_empty() {
        return fail(format!(
            "this compositor does not offer {}. It offered: {}",
            missing.join(" or "),
            if seen.is_empty() { "nothing".to_string() } else { seen.join(", ") }
        ));
    }

    Ok(Globals { compositor: compositor_id, shm: shm_id, wm_base: wm_base_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(m: Msg) -> (u32, u16, Vec<u8>) {
        let (buf, _) = m.finish();
        let object = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let word = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]);
        (object, (word & 0xffff) as u16, buf[8..].to_vec())
    }

    #[test]
    fn a_header_carries_the_length_and_the_opcode() {
        let (buf, _) = Msg::new(7, 3).uint(0xdeadbeef).finish();
        assert_eq!(buf.len(), 12);
        let word = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]);
        assert_eq!(word >> 16, 12, "length includes the header");
        assert_eq!(word & 0xffff, 3, "opcode survives the length being stamped");
    }

    #[test]
    fn an_empty_message_is_just_its_header() {
        let (buf, _) = Msg::new(1, 0).finish();
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn strings_carry_their_nul_and_pad_to_a_word() {
        // "wl_shm" is 6 bytes, 7 with the NUL, padded to 8.
        let (_, _, body) = body_of(Msg::new(1, 0).string("wl_shm"));
        assert_eq!(body.len(), 4 + 8);
        assert_eq!(u32::from_ne_bytes([body[0], body[1], body[2], body[3]]), 7);
        assert_eq!(&body[4..10], b"wl_shm");
        assert_eq!(body[10], 0, "the NUL must be there");
        assert_eq!(body[11], 0, "and the padding");
    }

    #[test]
    fn a_string_whose_length_is_already_a_multiple_of_four_still_pads() {
        // "abc" is 3, 4 with the NUL -- already a word, so no extra padding.
        let (_, _, body) = body_of(Msg::new(1, 0).string("abc"));
        assert_eq!(body.len(), 8);
        // "abcd" is 4, 5 with the NUL -- pads to 8.
        let (_, _, body) = body_of(Msg::new(1, 0).string("abcd"));
        assert_eq!(body.len(), 12);
    }

    // Written the wrong way round first: the spec counts the NUL, so an empty
    // string is length 1 and eight bytes, and only a *null* string is length 0.
    #[test]
    fn an_empty_string_is_a_nul_and_not_a_null() {
        let (_, _, body) = body_of(Msg::new(1, 0).string(""));
        assert_eq!(body.len(), 8, "four for the length, four for the padded NUL");
        assert_eq!(
            u32::from_ne_bytes([body[0], body[1], body[2], body[3]]),
            1,
            "the length counts the NUL"
        );
        assert_eq!(&body[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn bind_sends_the_interface_and_version_with_the_new_id() {
        // The generic new_id is the one shape that is not just a word, and
        // getting it wrong is the classic first Wayland bug.
        let (_, _, body) = body_of(Msg::new(2, registry::BIND).uint(9).bind_id("wl_shm", 1, 5));
        let e = Event { object: 2, opcode: registry::BIND, body };
        let mut a = e.args();
        assert_eq!(a.uint(), Some(9), "the global's name");
        assert_eq!(a.string().as_deref(), Some("wl_shm"));
        assert_eq!(a.uint(), Some(1), "the version");
        assert_eq!(a.uint(), Some(5), "the id we chose");
    }

    #[test]
    fn a_descriptor_takes_no_room_in_the_byte_stream() {
        let (buf, fds) = Msg::new(3, shm::CREATE_POOL).new_id(4).fd(7).int(64).finish();
        assert_eq!(fds, vec![7]);
        // header + new_id + size, and nothing for the descriptor.
        assert_eq!(buf.len(), 8 + 4 + 4);
    }

    #[test]
    fn arguments_read_back_in_order() {
        let (_, _, body) = body_of(
            Msg::new(1, 0).uint(1).int(-2).string("hi").uint(3),
        );
        let e = Event { object: 1, opcode: 0, body };
        let mut a = e.args();
        assert_eq!(a.uint(), Some(1));
        assert_eq!(a.int(), Some(-2));
        assert_eq!(a.string().as_deref(), Some("hi"));
        assert_eq!(a.uint(), Some(3));
        assert_eq!(a.uint(), None, "and then nothing");
    }

    #[test]
    fn reading_past_the_end_says_none_rather_than_panicking() {
        let e = Event { object: 1, opcode: 0, body: vec![1, 2] };
        let mut a = e.args();
        assert_eq!(a.uint(), None);
        assert_eq!(a.string(), None);
        assert_eq!(a.array(), None);
    }

    #[test]
    fn a_truncated_string_is_refused_rather_than_read_off_the_end() {
        // Claims 64 bytes and supplies none.
        let mut body = 64u32.to_ne_bytes().to_vec();
        body.extend_from_slice(&[0; 4]);
        let e = Event { object: 1, opcode: 0, body };
        assert_eq!(e.args().string(), None);
    }

    // --- the splitter, which is where a stream protocol actually goes wrong ---

    fn conn_for_test() -> (Conn, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        (Conn::wrap(a).unwrap(), b)
    }

    #[test]
    fn messages_split_on_their_own_lengths() {
        let (mut c, _peer) = conn_for_test();
        let (m1, _) = Msg::new(1, 0).uint(11).finish();
        let (m2, _) = Msg::new(2, 1).uint(22).uint(33).finish();
        c.inbuf.extend_from_slice(&m1);
        c.inbuf.extend_from_slice(&m2);
        c.split();
        let e1 = c.next_event().unwrap();
        assert_eq!((e1.object, e1.opcode), (1, 0));
        assert_eq!(e1.args().uint(), Some(11));
        let e2 = c.next_event().unwrap();
        assert_eq!((e2.object, e2.opcode), (2, 1));
        assert!(c.next_event().is_none());
    }

    #[test]
    fn half_a_message_waits_for_the_rest() {
        let (mut c, _peer) = conn_for_test();
        let (m, _) = Msg::new(5, 2).uint(1).uint(2).finish();
        let cut = m.len() - 3;
        c.inbuf.extend_from_slice(&m[..cut]);
        c.split();
        assert!(c.next_event().is_none(), "a partial message must not be delivered");
        c.inbuf.extend_from_slice(&m[cut..]);
        c.split();
        let e = c.next_event().expect("the rest completed it");
        assert_eq!((e.object, e.opcode), (5, 2));
    }

    #[test]
    fn fewer_than_eight_bytes_is_never_a_message() {
        let (mut c, _peer) = conn_for_test();
        c.inbuf.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
        c.split();
        assert!(c.next_event().is_none());
        assert_eq!(c.inbuf.len(), 7, "and it is kept for later");
    }

    #[test]
    fn a_nonsense_length_does_not_loop_forever() {
        // A length below the header would otherwise make the splitter drain
        // nothing and spin.
        let (mut c, _peer) = conn_for_test();
        c.inbuf.extend_from_slice(&1u32.to_ne_bytes());
        c.inbuf.extend_from_slice(&((4u32 << 16) | 0).to_ne_bytes());
        c.inbuf.extend_from_slice(&[0; 8]);
        c.split();
        assert!(c.next_event().is_none());
        assert!(c.inbuf.is_empty(), "the stream is dropped rather than trusted");
    }

    #[test]
    fn ids_start_above_the_display_and_never_repeat() {
        let (mut c, _peer) = conn_for_test();
        let a = c.allocate();
        let b = c.allocate();
        assert!(a > DISPLAY, "1 belongs to wl_display");
        assert_eq!(b, a + 1);
    }

    #[test]
    fn a_display_error_becomes_a_sentence() {
        let (_, _, body) = body_of(Msg::new(DISPLAY, 0).uint(12).uint(3).string("bad surface"));
        let e = Event { object: DISPLAY, opcode: display::EV_ERROR, body };
        let why = Conn::describe_error(&e).expect("an error on object 1 is an error");
        assert!(why.contains("bad surface"), "{}", why);
        assert!(why.contains("object 12"), "{}", why);
    }

    #[test]
    fn an_event_that_is_not_a_display_error_is_not_read_as_one() {
        let e = Event { object: 4, opcode: 0, body: vec![] };
        assert!(Conn::describe_error(&e).is_none());
    }

    // --- against a real compositor -------------------------------------------
    //
    // `rfb.rs` is proven on a socket rather than in arithmetic, and the same
    // applies here with more force: the wire format can be perfect and the
    // client still be wrong about ORDER -- committing before the first
    // configure, acking the wrong serial, answering an event that belongs to
    // another object. None of that is visible without a compositor, so these
    // tests run when one is present and step aside when it is not.

    fn compositor_present() -> bool {
        let display = match std::env::var("WAYLAND_DISPLAY") {
            Ok(d) => d,
            Err(_) => return false,
        };
        if display.starts_with('/') {
            return std::path::Path::new(&display).exists();
        }
        match std::env::var("XDG_RUNTIME_DIR") {
            Ok(dir) => std::path::Path::new(&dir).join(display).exists(),
            Err(_) => false,
        }
    }

    #[test]
    fn a_window_opens_and_the_compositor_takes_the_pixels() {
        if !compositor_present() {
            eprintln!("skipped: no compositor (set WAYLAND_DISPLAY to run this)");
            return;
        }
        let mut win = Window::open("orrery (test)", 640, 400).expect("the window opened");
        assert!(win.width > 0 && win.height > 0, "configured to {}x{}", win.width, win.height);

        win.fill(0xfc, 0xe2, 0xab);

        // The colour must actually be in the mapped buffer, in the byte order
        // XRGB8888 means on this machine.
        let expect = 0x00fc_e2_abu32.to_ne_bytes();
        assert_eq!(&win.pixels()[0..4], &expect, "fill did not reach the mapping");

        win.present().expect("presented");
        assert!(win.buffer_busy(), "the buffer is the compositor's until released");

        let start = std::time::Instant::now();
        while win.buffer_busy() && start.elapsed() < std::time::Duration::from_secs(5) {
            win.poll().expect("polled");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !win.buffer_busy(),
            "the compositor never released the buffer, so it never read the frame"
        );
    }

    #[test]
    fn a_window_larger_than_the_output_is_configured_down_and_follows() {
        if !compositor_present() {
            eprintln!("skipped: no compositor");
            return;
        }
        // Deliberately bigger than the headless output, so the compositor has
        // to answer with a size of its own and the resize path runs for real
        // rather than being simulated.
        let mut win = Window::open("orrery (resize)", 2400, 1600).expect("opened");
        win.fill(0x12, 0x34, 0x56);
        win.present().expect("presented");

        let start = std::time::Instant::now();
        let mut resized = false;
        while start.elapsed() < std::time::Duration::from_secs(3) {
            let c = win.poll().expect("polled");
            if c.resized {
                win.apply_resize().expect("resized");
                resized = true;
                win.fill(0x12, 0x34, 0x56);
                win.present().expect("re-presented");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Not every compositor overrides the requested size, so this asserts
        // the path is sound when it happens rather than that it must.
        if resized {
            assert!(win.width > 0 && win.height > 0);
            assert!(
                win.pixels().len() >= win.width * win.height * 4,
                "the mapping is smaller than the size it was resized to"
            );
        } else {
            eprintln!("note: this compositor accepted 2400x1600 unchanged");
        }
    }
}
