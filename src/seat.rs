//! The seat -- one node, full screen, in the terminal you are already in.
//!
//! §D.5 of `docs/fleet-lab-report.md` asks for "a live console on the object
//! itself, full-screen, openable in its own tab", and names the fleet's two as
//! the SSH session and the VNC session. This is the VNC one. It is `orrery
//! --seat NODE`, and it is not the web console: the wall is for eight nodes at
//! a glance and the seat is for one node you are working on, which is the same
//! split §III-B found when it priced eight live sessions and refused them.
//!
//! THE FOUR TIMBUKTU VERBS, and what is actually behind each:
//!
//!   Observe   the node's screen, read-only. Built.
//!   Control   Observe plus keyboard and pointer. Built.
//!   Exchange  the two-pane file browser. Not built -- see `EXCHANGE_WHY`.
//!   Terminal  a shell on the node. NOT BUILDABLE as the fleet stands, and the
//!             reason is a good one rather than a gap: see `TERMINAL_WHY`.
//!
//! THE READ MODEL IS STILL THE CLI'S. The address this dials came out of
//! `copal fleet state` and nowhere else -- `resolve` below is the only way in,
//! and it refuses a node the document does not list. §12's rule is about how
//! the console LEARNS things, and it is intact; what changes here is that the
//! seat, having been told where a node is, looks at it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fleet::Fleet;
use crate::json;
use crate::paint::{self, Mode as Paint};
use crate::rfb::{Input, Rfb, Screen, DEFAULT_PORT};

/// Why Exchange is not here.
const EXCHANGE_WHY: &str = "Exchange is a two-pane SFTP browser over the session the certificate \
already authorises. The certificate is there; the browser is not. L6.";

/// Why Terminal is not here, which is not the same kind of absence.
///
/// The node's forced command (`copal-prep.sh`, the `copal-fleet-exec` heredoc)
/// is a closed `case` over thirteen verbs with, in its own words, "no hatch, no
/// raw, and no pass-through". An interactive shell is exactly the pass-through
/// it refuses. So a Terminal face is not a missing feature in orrery -- it is a
/// second credential with a wider door, and that is a fleet decision rather
/// than a console one.
const TERMINAL_WHY: &str = "A shell on a node would need a second credential. The operator \
certificate lands in a forced command with thirteen verbs and no pass-through, which is the \
security boundary working. Adding Terminal means widening that door, in copal-prep.sh, \
deliberately -- not here.";

// ------------------------------------------------------------------- tty ---

/// Raw mode, the alternate screen, and putting all of it back.
///
/// Termios is a libc call and libc is a dependency, so the terminal is driven
/// the way everything else in this program drives the outside world: by
/// shelling out. `stty -g` hands back the whole mode as one opaque string,
/// which is a better save/restore than any set of flags this could reconstruct.
struct Tty {
    saved: Option<String>,
    restored: bool,
}

impl Tty {
    fn new() -> Result<Tty, String> {
        let saved = stty(&["-g"]).ok().map(|s| s.trim().to_string());
        if saved.is_none() {
            return Err("this is not a terminal -- the seat needs one".to_string());
        }
        // raw: no line discipline, no echo, no signal characters. The seat
        // wants Ctrl-C to reach the node rather than kill the seat.
        stty(&["raw", "-echo"])?;
        let mut t = Tty { saved, restored: false };
        t.write(concat!(
            "\x1b[?1049h", // alternate screen -- the operator's scrollback survives
            "\x1b[?25l",   // hide the cursor
            "\x1b[?1000h", // mouse: button events
            "\x1b[?1002h", // mouse: motion while a button is down, so drags work
            "\x1b[?1006h", // mouse: SGR coordinates, so past column 223 works
        ));
        Ok(t)
    }

    fn write(&mut self, s: &str) {
        let out = std::io::stdout();
        let mut h = out.lock();
        let _ = h.write_all(s.as_bytes());
        let _ = h.flush();
    }

    /// Rows and columns, asked fresh because the operator may have resized.
    ///
    /// A ZERO IS NOT A SIZE. A pty that nobody set a window size on -- which is
    /// what `script` and most CI harnesses hand you -- answers "0 0", and
    /// taking that literally put the seat in its too-small branch forever.
    /// Treat it the same as no answer at all.
    fn size(&self) -> (usize, usize) {
        if let Ok(s) = stty(&["size"]) {
            let mut it = s.split_whitespace();
            if let (Some(r), Some(c)) = (it.next(), it.next()) {
                if let (Ok(r), Ok(c)) = (r.parse::<usize>(), c.parse::<usize>()) {
                    if r > 0 && c > 0 {
                        return (r, c);
                    }
                }
            }
        }
        (24, 80)
    }

    /// Put the terminal back exactly as it was.
    ///
    /// Idempotent, because it runs from `Drop` and also from the panic path,
    /// and leaving a terminal in raw mode with the cursor hidden is the single
    /// rudest thing a program like this can do.
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        self.write(concat!(
            "\x1b[?1006l", "\x1b[?1002l", "\x1b[?1000l", // mouse off
            "\x1b[?25h",   // cursor back
            "\x1b[?1049l", // and off the alternate screen
        ));
        if let Some(saved) = self.saved.clone() {
            let _ = stty(&[&saved]);
        } else {
            let _ = stty(&["sane"]);
        }
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        self.restore();
    }
}

fn stty(args: &[&str]) -> Result<String, String> {
    let out = Command::new("stty")
        .args(args)
        // stdin must be the terminal: stty acts on the descriptor it is given.
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run stty: {}", e))?;
    if !out.status.success() {
        return Err("stty refused".to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ----------------------------------------------------------------- input ---

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    /// A printable character.
    Ch(char),
    /// Ctrl held with an ASCII letter, already decoded from the control byte.
    Ctrl(char),
    Key(u32),
    /// SGR mouse: column, row (1-based cells), button bitmask.
    Mouse { col: usize, row: usize, buttons: u8 },
    /// The answer to `\x1b[14t`: the window's size in pixels.
    PixelSize { w: usize, h: usize },
}

// X11 keysyms, which are what RFB speaks.
const K_BACKSPACE: u32 = 0xff08;
const K_TAB: u32 = 0xff09;
const K_RETURN: u32 = 0xff0d;
const K_ESCAPE: u32 = 0xff1b;
const K_HOME: u32 = 0xff50;
const K_LEFT: u32 = 0xff51;
const K_UP: u32 = 0xff52;
const K_RIGHT: u32 = 0xff53;
const K_DOWN: u32 = 0xff54;
const K_PRIOR: u32 = 0xff55;
const K_NEXT: u32 = 0xff56;
const K_END: u32 = 0xff57;
const K_DELETE: u32 = 0xffff;
const K_CONTROL_L: u32 = 0xffe3;

/// Pull one event off the front of `buf`.
///
/// Returns `None` when what is there is a prefix of something longer, which
/// happens constantly: an escape sequence can arrive split across two reads,
/// and treating a lone `\x1b` as Escape would turn every arrow key into a
/// spurious keypress on the node.
fn parse(buf: &mut Vec<u8>) -> Option<Ev> {
    if buf.is_empty() {
        return None;
    }
    if buf[0] != 0x1b {
        let b = buf.remove(0);
        return Some(match b {
            b'\r' | b'\n' => Ev::Key(K_RETURN),
            b'\t' => Ev::Key(K_TAB),
            0x7f | 0x08 => Ev::Key(K_BACKSPACE),
            // Ctrl-A..Ctrl-Z arrive as 1..26. Ctrl-I and Ctrl-M are Tab and
            // Return and were taken above, which is the right precedence.
            0x01..=0x1a => Ev::Ctrl((b - 1 + b'a') as char),
            0x1c..=0x1f => Ev::Ctrl((b - 0x1c + b'\\') as char),
            _ => {
                // UTF-8: put the byte back and decode a whole character.
                buf.insert(0, b);
                return take_utf8(buf);
            }
        });
    }

    // A bare ESC with nothing after it yet: wait, unless it is clearly alone.
    if buf.len() == 1 {
        return None;
    }
    if buf[1] != b'[' && buf[1] != b'O' {
        buf.remove(0);
        return Some(Ev::Key(K_ESCAPE));
    }
    if buf.len() == 2 {
        return None;
    }

    // SGR mouse: ESC [ < b ; col ; row (M|m)
    if buf[1] == b'[' && buf[2] == b'<' {
        let end = buf.iter().position(|&c| c == b'M' || c == b'm')?;
        let body = String::from_utf8_lossy(&buf[3..end]).to_string();
        let fin = buf[end];
        buf.drain(..=end);
        let n: Vec<usize> = body.split(';').filter_map(|p| p.parse().ok()).collect();
        if n.len() != 3 {
            return None;
        }
        // Bit 5 (32) marks motion; bits 0-1 pick the button; 'm' is a release.
        let raw = n[0];
        let buttons = if fin == b'm' {
            0
        } else if raw & 64 != 0 {
            // Wheel: VNC calls these buttons 4 and 5.
            if raw & 1 == 0 { 8 } else { 16 }
        } else {
            match raw & 3 {
                0 => 1,
                1 => 2,
                2 => 4,
                _ => 0,
            }
        };
        return Some(Ev::Mouse { col: n[1], row: n[2], buttons });
    }

    // Everything else: ESC [ ... final, or ESC O final.
    let end = buf
        .iter()
        .skip(2)
        .position(|&c| c.is_ascii_alphabetic() || c == b'~')
        .map(|i| i + 2)?;
    let body = String::from_utf8_lossy(&buf[2..end]).to_string();
    let fin = buf[end];
    buf.drain(..=end);

    // The pixel-size report: ESC [ 4 ; height ; width t
    if fin == b't' {
        let n: Vec<usize> = body.split(';').filter_map(|p| p.parse().ok()).collect();
        if n.len() == 3 && n[0] == 4 {
            return Some(Ev::PixelSize { w: n[2], h: n[1] });
        }
        return None;
    }

    Some(Ev::Key(match fin {
        b'A' => K_UP,
        b'B' => K_DOWN,
        b'C' => K_RIGHT,
        b'D' => K_LEFT,
        b'H' => K_HOME,
        b'F' => K_END,
        b'~' => match body.split(';').next().unwrap_or("") {
            "1" | "7" => K_HOME,
            "2" => 0xff63, // Insert
            "3" => K_DELETE,
            "4" | "8" => K_END,
            "5" => K_PRIOR,
            "6" => K_NEXT,
            // F5..F12 are 15,17..21,23,24 and land on keysyms 0xffc2..
            "15" => 0xffc2,
            "17" => 0xffc3,
            "18" => 0xffc4,
            "19" => 0xffc5,
            "20" => 0xffc6,
            "21" => 0xffc7,
            "23" => 0xffc8,
            "24" => 0xffc9,
            _ => return None,
        },
        // ESC O P..S are F1..F4.
        b'P' => 0xffbe,
        b'Q' => 0xffbf,
        b'R' => 0xffc0,
        b'S' => 0xffc1,
        _ => return None,
    }))
}

/// Decode one UTF-8 character, or wait if it is not all here yet.
fn take_utf8(buf: &mut Vec<u8>) -> Option<Ev> {
    let need = match buf[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A stray continuation byte: drop it rather than stall forever.
        _ => {
            buf.remove(0);
            return None;
        }
    };
    if buf.len() < need {
        return None;
    }
    let bytes: Vec<u8> = buf.drain(..need).collect();
    match std::str::from_utf8(&bytes) {
        Ok(s) => s.chars().next().map(Ev::Ch),
        Err(_) => None,
    }
}

/// A keysym for a printable character.
///
/// Latin-1 is the identity, which covers most of what a museum operator types.
/// Everything above it uses the Unicode range RFB borrowed from X11.
fn keysym_for(c: char) -> u32 {
    let u = c as u32;
    if u < 0x100 {
        u
    } else {
        0x0100_0000 + u
    }
}

// ------------------------------------------------------------------ modes ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum Face {
    Observe,
    Control,
    Exchange,
    Terminal,
}

impl Face {
    fn label(&self) -> &'static str {
        match self {
            Face::Observe => "observe",
            Face::Control => "control",
            Face::Exchange => "exchange",
            Face::Terminal => "terminal",
        }
    }
}

// ------------------------------------------------------------------- node ---

/// One node, as the read model describes it.
#[derive(Debug)]
pub struct Node {
    pub id: String,
    pub address: String,
    pub status: String,
    pub scene: String,
    pub temp: String,
}

/// Find a node in `copal fleet state`, or say what is actually there.
///
/// THIS IS THE ONLY WAY THE SEAT LEARNS AN ADDRESS. A node id typed on the
/// command line is a candidate; a node id in the state document has been
/// through the certificate check. `verbs.rs` makes the same distinction in the
/// same words for the same reason.
pub fn resolve(fleet: &Fleet, id: &str) -> Result<Node, String> {
    let doc = fleet.state()?;
    let v = json::parse(&doc).map_err(|e| format!("unreadable JSON from the CLI: {}", e))?;
    let nodes = v
        .get("nodes")
        .and_then(|n| n.as_array())
        .ok_or_else(|| "the state document has no nodes".to_string())?;

    let field = |n: &json::Value, k: &str| {
        n.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };

    for n in nodes {
        if field(n, "id") == id {
            let address = field(n, "address");
            if address.is_empty() {
                return Err(format!("{} is in the fleet but has no address yet", id));
            }
            return Ok(Node {
                id: id.to_string(),
                address,
                status: field(n, "status"),
                scene: field(n, "scene"),
                temp: n
                    .get("temp")
                    .map(|t| match t {
                        json::Value::Num(f) => format!("{}\u{b0}C", *f as i64),
                        other => other.as_str().unwrap_or("").to_string(),
                    })
                    .unwrap_or_default(),
            });
        }
    }

    let known: Vec<String> = nodes
        .iter()
        .filter_map(|n| n.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .collect();
    Err(format!(
        "no node called {} in this fleet. There is: {}",
        id,
        known.join(", ")
    ))
}

// ------------------------------------------------------------------- seat ---

/// The shared picture. The pump thread writes it; the draw loop reads it.
struct Frame {
    screen: Screen,
    /// Bumped on every change, so the draw loop can skip a frame that would be
    /// identical -- which on a gallery node showing a still scene is nearly
    /// all of them, and re-encoding sixel for no reason is the one thing here
    /// that could actually cost a Zero 2 something.
    version: u64,
    error: Option<String>,
}

pub struct Opts {
    pub port: u16,
    pub paint: Option<Paint>,
    /// Frames per second to *ask* for. The node decides what it can give.
    pub fps: u32,
}

impl Default for Opts {
    fn default() -> Self {
        // Six is enough to see a menu open and cheap enough that a board
        // running the exhibit does not notice. §III-B's whole finding is that
        // frame rate is the expensive axis, so this is the knob that is low.
        Opts { port: DEFAULT_PORT, paint: None, fps: 6 }
    }
}

pub fn run(fleet: &Fleet, id: &str, opts: Opts) -> Result<(), String> {
    let node = resolve(fleet, id)?;

    eprintln!("orrery: dialling {} at {}:{}", node.id, node.address, opts.port);
    let rfb = Rfb::connect(&node.address, opts.port).map_err(|e| e.to_string())?;
    let info_name = rfb.info.name.clone();
    let (fw, fh) = (rfb.info.w, rfb.info.h);
    let input = rfb.input();

    let frame = Arc::new(Mutex::new(Frame {
        screen: Screen::new(fw, fh),
        version: 0,
        error: None,
    }));

    // The pump. It owns the read half and blocks on it, which is why it is a
    // thread: a still screen sends nothing for minutes and the seat still has
    // to answer the keyboard.
    let pump_frame = Arc::clone(&frame);
    let interval = Duration::from_millis((1000 / opts.fps.max(1)) as u64);
    std::thread::spawn(move || pump(rfb, pump_frame, interval));

    let mut tty = Tty::new()?;
    let stdin_rx = stdin_thread();

    // Ask the terminal how big it is in pixels. Best effort: terminals that do
    // not answer simply never send the reply and the heuristic stands.
    tty.write("\x1b[14t");

    let res = draw_loop(&mut tty, &stdin_rx, &frame, &input, &node, &info_name, &opts);
    // Restore before anything is printed, so an error message lands on the
    // operator's real screen and not on the alternate one that is about to go.
    input.release_all();
    tty.restore();
    res
}

fn pump(mut rfb: Rfb, frame: Arc<Mutex<Frame>>, interval: Duration) {
    if let Err(e) = rfb.request_update(false) {
        if let Ok(mut f) = frame.lock() {
            f.error = Some(e.to_string());
        }
        return;
    }
    let mut last_request = Instant::now();
    loop {
        match rfb.pump() {
            Ok(changed) => {
                if changed {
                    if let Ok(mut f) = frame.lock() {
                        // Copy rather than share: the draw loop scales and
                        // encodes, which takes long enough that holding the
                        // lock across it would stall the pump.
                        f.screen.w = rfb.screen.w;
                        f.screen.h = rfb.screen.h;
                        f.screen.px.clear();
                        f.screen.px.extend_from_slice(&rfb.screen.px);
                        f.version += 1;
                    }
                }
            }
            Err(e) => {
                if let Ok(mut f) = frame.lock() {
                    f.error = Some(e.to_string());
                }
                return;
            }
        }
        // Ask for the next one. Pacing this rather than the drawing is what
        // keeps the node's encoder off the CPU it needs for the exhibit.
        let since = last_request.elapsed();
        if since < interval {
            std::thread::sleep(interval - since);
        }
        last_request = Instant::now();
        if rfb.request_update(true).is_err() {
            return;
        }
    }
}

/// Stdin on its own thread, because there is no portable non-blocking read
/// without libc and this program does not have libc.
fn stdin_thread() -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    rx
}

#[allow(clippy::too_many_arguments)]
fn draw_loop(
    tty: &mut Tty,
    stdin_rx: &Receiver<Vec<u8>>,
    frame: &Arc<Mutex<Frame>>,
    input: &Input,
    node: &Node,
    desktop: &str,
    opts: &Opts,
) -> Result<(), String> {
    let paint_mode = opts.paint.unwrap_or_else(Paint::detect);
    let mut face = Face::Observe;
    let mut pending: Vec<u8> = Vec::new();
    let mut drawn_version = u64::MAX;
    let mut drawn_size = (0usize, 0usize);
    let mut note: Option<String> = None;
    // Until the terminal answers `\x1b[14t`, assume a cell of 8x17 -- close to
    // every default monospace setup, and only the sixel path uses it.
    let mut cell = (8usize, 17usize);
    let mut cell_known = false;
    // Where the image actually sits, so a mouse click can be turned back into
    // a coordinate on the node's screen.
    let mut view = (0usize, 0usize, 1usize, 1usize); // col0, row0, cols, rows

    loop {
        let (rows, cols) = tty.size();
        if rows < 6 || cols < 20 {
            // STILL READ THE KEYBOARD. An earlier version returned to the top
            // of the loop here, which meant a window too small to draw in was
            // also a window the operator could not quit -- and raw mode has
            // already taken Ctrl-C away from them. Being stuck inside a
            // machine is the one thing a seat may not do.
            tty.write("\x1b[H\x1b[2Jthe seat needs a larger window -- q to quit\r\n");
            match stdin_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => pending.extend_from_slice(&chunk),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
            while let Some(ev) = parse(&mut pending) {
                if ev == Ev::Ch('q') || ev == Ev::Ctrl('c') {
                    return Ok(());
                }
            }
            continue;
        }

        // --- draw ---------------------------------------------------------
        let (version, err, picture) = {
            let f = frame.lock().map_err(|_| "the frame thread failed".to_string())?;
            let need = f.version != drawn_version || (rows, cols) != drawn_size;
            let pic = if need && f.error.is_none() {
                let img_rows = rows.saturating_sub(4);
                view = (1, 3, cols, img_rows);
                Some(match face {
                    // A FACE THAT IS NOT BUILT SHOWS WHY, IN THE PANE. The web
                    // seat does exactly this and for the same reason: a
                    // truncated sentence in a status line is worse than no
                    // sentence, and showing the node's screen under a tab that
                    // does nothing implies the tab does something.
                    Face::Exchange => pane(EXCHANGE_WHY, cols, img_rows),
                    Face::Terminal => pane(TERMINAL_WHY, cols, img_rows),
                    _ => match paint_mode {
                        Paint::HalfBlock => paint::halfblock(&f.screen, cols, img_rows),
                        Paint::Sixel => {
                            paint::sixel(&f.screen, cols * cell.0, img_rows * cell.1)
                        }
                    },
                })
            } else {
                None
            };
            (f.version, f.error.clone(), pic)
        };

        if let Some(pic) = picture {
            let mut out = String::with_capacity(pic.len() + 512);
            out.push_str("\x1b[H");
            out.push_str(&header(node, desktop, face, cols));
            out.push_str("\x1b[3;1H");
            out.push_str(&pic);
            out.push_str("\x1b[0J"); // clear whatever the last frame left below
            out.push_str(&format!("\x1b[{};1H", rows));
            out.push_str(&footer(face, note.as_deref(), paint_mode, cols));
            tty.write(&out);
            drawn_version = version;
            drawn_size = (rows, cols);
        }

        if let Some(e) = err {
            return Err(e);
        }

        // --- input --------------------------------------------------------
        // A short wait: long enough not to spin, short enough that a keystroke
        // does not feel posted.
        match stdin_rx.recv_timeout(Duration::from_millis(30)) {
            Ok(chunk) => pending.extend_from_slice(&chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        while let Some(ev) = parse(&mut pending) {
            if let Ev::PixelSize { w, h } = ev {
                if w > 0 && h > 0 && cols > 0 && rows > 0 && !cell_known {
                    cell = ((w / cols).max(1), (h / rows).max(1));
                    cell_known = true;
                    drawn_version = u64::MAX; // redraw at the real size
                }
                continue;
            }

            if face == Face::Control {
                // EVERYTHING GOES TO THE NODE except the one way out. Ctrl-]
                // is telnet's escape and has been for forty years, so it is
                // the key an operator is least surprised by and least likely
                // to need on a museum desktop.
                if ev == Ev::Ctrl(']') {
                    input.release_all();
                    face = Face::Observe;
                    note = Some("control released".to_string());
                    drawn_version = u64::MAX;
                    continue;
                }
                if let Err(e) = forward(input, &ev, &view, frame) {
                    return Err(e);
                }
                continue;
            }

            // Not controlling: the seat's own keys.
            match ev {
                Ev::Ch('q') | Ev::Ctrl('c') => return Ok(()),
                Ev::Ch('o') => {
                    face = Face::Observe;
                    note = None;
                }
                Ev::Ch('c') => {
                    face = Face::Control;
                    note = Some("control -- Ctrl-] to let go".to_string());
                }
                Ev::Ch('e') => {
                    face = Face::Exchange;
                    note = None;
                }
                Ev::Ch('t') => {
                    face = Face::Terminal;
                    note = None;
                }
                Ev::Ch('r') => note = None,
                _ => continue,
            }
            drawn_version = u64::MAX;
        }
    }
}

/// Send one event to the node.
fn forward(
    input: &Input,
    ev: &Ev,
    view: &(usize, usize, usize, usize),
    frame: &Arc<Mutex<Frame>>,
) -> Result<(), String> {
    let r = match ev {
        Ev::Ch(c) => input.tap(keysym_for(*c)),
        Ev::Ctrl(c) => input.chord(&[K_CONTROL_L], keysym_for(*c)),
        Ev::Key(k) => input.tap(*k),
        Ev::Mouse { col, row, buttons } => {
            // A cell maps to a rectangle of node pixels; aim at its middle.
            let (c0, r0, cw, ch) = *view;
            let (fw, fh) = match frame.lock() {
                Ok(f) => (f.screen.w, f.screen.h),
                Err(_) => return Err("the frame thread failed".to_string()),
            };
            if cw == 0 || ch == 0 || fw == 0 || fh == 0 {
                return Ok(());
            }
            let dx = col.saturating_sub(c0).min(cw.saturating_sub(1));
            let dy = row.saturating_sub(r0).min(ch.saturating_sub(1));
            let x = (dx * 2 + 1) * fw / (cw * 2);
            let y = (dy * 2 + 1) * fh / (ch * 2);
            input.pointer(x.min(fw - 1), y.min(fh - 1), *buttons)
        }
        Ev::PixelSize { .. } => Ok(()),
    };
    r.map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ chrome ---

fn header(node: &Node, desktop: &str, face: Face, cols: usize) -> String {
    let dot = match node.status.as_str() {
        "up" => "\x1b[32m\u{25cf}\x1b[0m",
        "down" => "\x1b[90m\u{25cb}\x1b[0m",
        _ => "\x1b[33m\u{25cf}\x1b[0m",
    };
    let facts: Vec<String> = [
        node.status.clone(),
        node.temp.clone(),
        if node.scene.is_empty() { String::new() } else { format!("scene {}", node.scene) },
        desktop.to_string(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    let left = format!("{} \x1b[1m{}\x1b[0m  \x1b[2m{}\x1b[0m", dot, node.id, facts.join(" \u{b7} "));
    let tabs = [Face::Observe, Face::Control, Face::Exchange, Face::Terminal]
        .iter()
        .map(|f| {
            let key = f.label().chars().next().unwrap();
            if *f == face {
                format!("\x1b[7m {} {} \x1b[0m", key, f.label())
            } else {
                format!("\x1b[2m {} {} \x1b[0m", key, f.label())
            }
        })
        .collect::<Vec<_>>()
        .join("");
    format!("{}\x1b[K\r\n{}\x1b[K\r\n", clip(&left, cols), clip(&tabs, cols))
}

fn footer(face: Face, note: Option<&str>, paint: Paint, cols: usize) -> String {
    let hint = match face {
        Face::Control => "Ctrl-] releases",
        _ => "o observe \u{b7} c control \u{b7} e exchange \u{b7} t terminal \u{b7} q quit",
    };
    let mode = match paint {
        Paint::Sixel => "sixel",
        Paint::HalfBlock => "half-block",
    };
    let line = match note {
        Some(n) => format!("\x1b[33m{}\x1b[0m", n),
        None => format!("\x1b[2m{}  \u{b7}  {}\x1b[0m", hint, mode),
    };
    format!("{}\x1b[K", clip(&line, cols))
}

/// A paragraph, word-wrapped and set in the middle of the pane.
///
/// Used for the faces that are not built. Sixty columns is about as wide as
/// prose stays readable, so it does not stretch to fill a gallery screen.
fn pane(text: &str, cols: usize, rows: usize) -> String {
    let width = cols.saturating_sub(4).min(60).max(10);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }

    let top = rows.saturating_sub(lines.len()) / 2;
    let left = cols.saturating_sub(width) / 2;
    let mut out = String::new();
    for r in 0..rows {
        if r > 0 {
            out.push_str("\r\n");
        }
        if r >= top && r - top < lines.len() {
            let l = &lines[r - top];
            out.push_str(&" ".repeat(left));
            out.push_str("\x1b[2m");
            out.push_str(l);
            out.push_str("\x1b[0m");
        }
        out.push_str("\x1b[K");
    }
    out
}

/// Cut a string to `cols` *printed* columns, counting escape sequences as
/// nothing. Naively truncating would slice a colour escape in half and leave
/// the rest of the terminal wearing it.
fn clip(s: &str, cols: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut width = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            for e in chars.by_ref() {
                out.push(e);
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        if width >= cols {
            break;
        }
        out.push(c);
        width += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evs(bytes: &[u8]) -> Vec<Ev> {
        let mut buf = bytes.to_vec();
        let mut out = Vec::new();
        while let Some(e) = parse(&mut buf) {
            out.push(e);
        }
        out
    }

    #[test]
    fn plain_characters() {
        assert_eq!(evs(b"ab"), vec![Ev::Ch('a'), Ev::Ch('b')]);
    }

    #[test]
    fn control_bytes_become_chords() {
        // 0x03 is Ctrl-C, and it must reach the node as Ctrl plus 'c' rather
        // than as a byte no X server has a keysym for.
        assert_eq!(evs(&[0x03]), vec![Ev::Ctrl('c')]);
        assert_eq!(evs(&[0x1d]), vec![Ev::Ctrl(']')]);
    }

    #[test]
    fn tab_and_return_win_over_their_control_codes() {
        assert_eq!(evs(b"\r"), vec![Ev::Key(K_RETURN)]);
        assert_eq!(evs(b"\t"), vec![Ev::Key(K_TAB)]);
        assert_eq!(evs(&[0x7f]), vec![Ev::Key(K_BACKSPACE)]);
    }

    #[test]
    fn arrows() {
        assert_eq!(
            evs(b"\x1b[A\x1b[B\x1b[C\x1b[D"),
            vec![Ev::Key(K_UP), Ev::Key(K_DOWN), Ev::Key(K_RIGHT), Ev::Key(K_LEFT)]
        );
    }

    #[test]
    fn function_and_navigation_keys() {
        assert_eq!(evs(b"\x1b[5~"), vec![Ev::Key(K_PRIOR)]);
        assert_eq!(evs(b"\x1b[3~"), vec![Ev::Key(K_DELETE)]);
        assert_eq!(evs(b"\x1bOP"), vec![Ev::Key(0xffbe)]);
    }

    // The bug this test exists for: an escape sequence split across two reads
    // used to emit a spurious Escape keypress into the node.
    #[test]
    fn a_split_escape_sequence_waits_rather_than_guessing() {
        let mut buf = b"\x1b".to_vec();
        assert_eq!(parse(&mut buf), None, "a lone ESC must wait");
        buf.extend_from_slice(b"[");
        assert_eq!(parse(&mut buf), None, "ESC [ must still wait");
        buf.extend_from_slice(b"A");
        assert_eq!(parse(&mut buf), Some(Ev::Key(K_UP)));
        assert!(buf.is_empty());
    }

    #[test]
    fn a_split_utf8_character_waits() {
        // U+00E9 is two bytes.
        let mut buf = vec![0xc3];
        assert_eq!(parse(&mut buf), None);
        buf.push(0xa9);
        assert_eq!(parse(&mut buf), Some(Ev::Ch('\u{e9}')));
    }

    #[test]
    fn sgr_mouse_press_and_release() {
        assert_eq!(
            evs(b"\x1b[<0;10;5M"),
            vec![Ev::Mouse { col: 10, row: 5, buttons: 1 }]
        );
        assert_eq!(
            evs(b"\x1b[<0;10;5m"),
            vec![Ev::Mouse { col: 10, row: 5, buttons: 0 }]
        );
    }

    #[test]
    fn sgr_mouse_right_button_and_wheel() {
        assert_eq!(evs(b"\x1b[<2;1;1M"), vec![Ev::Mouse { col: 1, row: 1, buttons: 4 }]);
        assert_eq!(evs(b"\x1b[<64;1;1M"), vec![Ev::Mouse { col: 1, row: 1, buttons: 8 }]);
        assert_eq!(evs(b"\x1b[<65;1;1M"), vec![Ev::Mouse { col: 1, row: 1, buttons: 16 }]);
    }

    #[test]
    fn mouse_coordinates_past_column_223_survive() {
        // The whole reason for asking for SGR mode rather than the original
        // encoding, which could not express a column above 223.
        assert_eq!(
            evs(b"\x1b[<0;400;300M"),
            vec![Ev::Mouse { col: 400, row: 300, buttons: 1 }]
        );
    }

    #[test]
    fn the_pixel_size_report_is_recognised() {
        assert_eq!(evs(b"\x1b[4;800;1200t"), vec![Ev::PixelSize { w: 1200, h: 800 }]);
    }

    #[test]
    fn keysyms_for_printable_characters() {
        assert_eq!(keysym_for('a'), 0x61);
        assert_eq!(keysym_for('A'), 0x41);
        assert_eq!(keysym_for('\u{e9}'), 0xe9);
        // Outside Latin-1, RFB uses X11's Unicode offset.
        assert_eq!(keysym_for('\u{20ac}'), 0x0100_20ac);
    }

    #[test]
    fn clip_counts_printed_columns_not_bytes() {
        let s = "\x1b[1mhello\x1b[0m world";
        // Five visible characters, whatever the escapes cost.
        assert_eq!(clip(s, 5).matches("hello").count(), 1);
        assert!(!clip(s, 5).contains("world"));
    }

    #[test]
    fn clip_never_cuts_an_escape_in_half() {
        let out = clip("\x1b[31mabc", 2);
        assert!(out.starts_with("\x1b[31m"), "escape was sliced: {:?}", out);
    }

    #[test]
    fn clip_leaves_a_short_string_alone() {
        assert_eq!(clip("abc", 10), "abc");
    }

    #[test]
    fn a_pane_wraps_and_never_overflows_the_width() {
        let out = pane(TERMINAL_WHY, 80, 20);
        for line in out.split("\r\n") {
            assert!(clip(line, 80) == line.to_string() || line.len() >= 80,
                    "a pane line escaped the width: {:?}", line);
        }
        assert_eq!(out.split("\r\n").count(), 20, "a pane must fill its rows");
    }

    #[test]
    fn a_pane_says_the_whole_sentence() {
        // The bug this replaces: the reason was put in the one-line footer and
        // came out cut off at "thirteen verbs".
        let out = pane(TERMINAL_WHY, 100, 24);
        let flat: String = out
            .replace("\r\n", " ")
            .replace("\x1b[2m", "")
            .replace("\x1b[0m", "")
            .replace("\x1b[K", "");
        for word in ["credential", "forced", "pass-through", "copal-prep.sh"] {
            assert!(flat.contains(word), "{:?} missing from the pane", word);
        }
    }

    #[test]
    fn a_pane_survives_a_tiny_window() {
        let out = pane(EXCHANGE_WHY, 12, 3);
        assert_eq!(out.split("\r\n").count(), 3);
    }

    #[test]
    fn resolve_refuses_a_node_the_read_model_does_not_list() {
        let fleet = Fleet::new(vec!["copal".into(), "fleet".into()], String::new(), true);
        let e = resolve(&fleet, "museum-99").unwrap_err();
        assert!(e.contains("no node called museum-99"), "{}", e);
        // And it says what there is, because the next thing the operator does
        // is retype the name.
        assert!(e.contains("museum-01"), "{}", e);
    }

    #[test]
    fn resolve_finds_a_node_and_its_address() {
        let fleet = Fleet::new(vec!["copal".into(), "fleet".into()], String::new(), true);
        let n = resolve(&fleet, "museum-01").unwrap();
        assert_eq!(n.id, "museum-01");
        assert!(!n.address.is_empty(), "a resolved node must carry an address");
    }

    #[test]
    fn resolve_refuses_a_node_with_no_address() {
        // museum-07 is the fixture's declared-but-never-announced node, which
        // is exactly the case that must not produce a dial to the empty string.
        let fleet = Fleet::new(vec!["copal".into(), "fleet".into()], String::new(), true);
        match resolve(&fleet, "museum-07") {
            Ok(n) => assert!(!n.address.is_empty()),
            Err(e) => assert!(e.contains("address"), "{}", e),
        }
    }
}
