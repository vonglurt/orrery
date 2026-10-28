//! Sessions: the fleet's terminals in a grid, each a resumable session.
//!
//! A tile a machine, from the fleet's static list and this one. On each, a
//! session named `orrery` in tmux (screen where there is no tmux), started
//! with `copal-tm` where the machine has it and a login shell after. The
//! session lives on the machine, so closing orrery loses nothing and any
//! terminal can join it: `ssh -t user@HOST tmux attach -t orrery`.
//!
//! WHY THE MULTIPLEXER DRAWS AND NOT ORRERY. A terminal program (copal-tm,
//! top, an editor) addresses the cursor; drawing it needs a terminal
//! emulator, and tmux already is one. So a tile is tmux's own picture of the
//! screen (`capture-pane -p`), fetched about once a second, and what is typed
//! into a focused tile is handed back to it (`send-keys`). One ssh connection
//! a machine is kept open (ControlMaster) so the polling is cheap.
//!
//! The text is 6x10 (font.rs F6X10), about 8 point, so six machines fit on one
//! screen and stay readable. Click a tile to type into it; Enlarge gives it
//! the whole body, Grid puts it back.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::font::{F10X20, F6X10, F8X13};
use crate::nav;
use crate::surface::Sym;
use crate::ui::{rect, Rect, Ui};

const HEADER_H: i32 = 32;
const PAD: i32 = 10;
const TITLE_H: i32 = 18;
const SESSION: &str = "orrery";

/// What a tile's worker needs to do next.
enum Ask {
    /// Literal text to type.
    Text(String),
    /// A key by tmux's name for it (Enter, BSpace, C-c, Up ...).
    Key(&'static str, Option<char>),
    /// The size the tile can show, in characters.
    Size(u16, u16),
    /// A command in a new window of the session, named (recipe-TIME), which the
    /// tile then shows (machines.rs: a recipe's command, its password typed
    /// by the person here). Its own window, so copal-tm in the first one is
    /// not typed into; closing it brings copal-tm back.
    Window(String, String),
}

pub struct Tile {
    pub name: String,
    /// "user@host", or "-" for this machine.
    pub target: String,
    lines: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<String>>,
    tx: Sender<Ask>,
    size: (u16, u16),
}

#[derive(Default)]
pub struct Sessions {
    pub tiles: Vec<Tile>,
    pub focus: Option<usize>,
    pub big: Option<usize>,
}

/// The ssh options every call shares: one master connection a machine.
fn ssh_args(target: &str) -> Vec<String> {
    vec![
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "ConnectTimeout=8".into(),
        "-o".into(), "ControlMaster=auto".into(),
        "-o".into(), "ControlPath=/tmp/orrery-ssh-%r@%h-%p".into(),
        "-o".into(), "ControlPersist=300".into(),
        target.into(),
    ]
}

/// Run a shell script on the machine (bash where there is one), its output.
fn on(target: &str, script: &str) -> Result<String, String> {
    let mut cmd = if target == "-" {
        let mut c = std::process::Command::new("bash");
        c.arg("-c").arg(format!("PATH=$PATH:/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin; {}", script));
        c
    } else {
        let mut c = std::process::Command::new("ssh");
        c.args(ssh_args(target));
        // The login shell may be busybox sh (nasser); the script is plain sh.
        c.arg(format!("PATH=$PATH:$HOME/.local/bin; {}", script));
        c
    };
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().lines().last().unwrap_or("failed").to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Single quotes for sh.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The machines: the fleet's static list (~/.copal/fleets/NAME/nodes), then this one.
pub fn machines(fleet: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    if let Ok(text) = std::fs::read_to_string(format!("{}/.copal/fleets/{}/nodes", home, fleet)) {
        for l in text.lines() {
            let l = l.split('#').next().unwrap_or("");
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() >= 2 {
                out.push((f[0].to_string(), format!("user@{}", f[1])));
            }
        }
    }
    out.push(("mac".to_string(), "-".to_string()));
    out
}

impl Sessions {
    /// A tile a machine, each with its worker. Nothing is started on a machine
    /// until its worker first runs: then the session is joined, or made.
    pub fn open(fleet: &str) -> Sessions {
        let mut s = Sessions::default();
        for (name, target) in machines(fleet) {
            s.add(name, target);
        }
        s
    }

    fn add(&mut self, name: String, target: String) -> usize {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new("connecting".to_string()));
        let (tx, rx) = channel();
        let (l2, s2, t2) = (Arc::clone(&lines), Arc::clone(&status), target.clone());
        thread::spawn(move || worker(&t2, rx, l2, s2));
        self.tiles.push(Tile { name, target, lines, status, tx, size: (0, 0) });
        self.tiles.len() - 1
    }

    /// Run a command in a machine's session, in its own window, and put that
    /// tile in front, enlarged and taking the keyboard: a machine not in the
    /// grid yet (a new one, from the Machines view) gets a tile.
    pub fn run(&mut self, name: &str, target: &str, command: &str) {
        let i = match self.tiles.iter().position(|t| t.name == name) {
            Some(i) => i,
            None => self.add(name.to_string(), target.to_string()),
        };
        // A name of its own: two windows named alike, and the second Apply's
        // typing reached the first one's prompt (28 October, boburner).
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = self.tiles[i].tx.send(Ask::Window(format!("recipe-{}", secs), command.to_string()));
        self.big = Some(i);
        self.focus = Some(i);
    }
}

/// Make or join the session; then, about once a second, type what is waiting
/// and fetch the screen.
fn worker(target: &str, rx: Receiver<Ask>, lines: Arc<Mutex<Vec<String>>>, status: Arc<Mutex<String>>) {
    let say = |m: String| *status.lock().unwrap_or_else(|e| e.into_inner()) = m;
    // copal-tm first where the machine has it; a login shell after, so the
    // session outlives it.
    let start = "sh -c 'command -v copal-tm >/dev/null 2>&1 && copal-tm; exec ${SHELL:-sh} -l'";
    let make = format!(
        "if command -v tmux >/dev/null 2>&1; then tmux has-session -t {s} 2>/dev/null || tmux new-session -d -s {s} -x 100 -y 30 {c}; echo tmux; \
         elif command -v screen >/dev/null 2>&1; then screen -ls {s} | grep -q '\\.{s}' || screen -dmS {s} sh -c {c2}; echo screen; \
         else echo none; fi",
        s = SESSION,
        c = q(start),
        c2 = q("command -v copal-tm >/dev/null 2>&1 && copal-tm; exec ${SHELL:-sh} -l"),
    );
    let kind = loop {
        match on(target, &make) {
            Ok(k) => break k.trim().to_string(),
            Err(e) => {
                say(format!("cannot reach it: {}", e));
                thread::sleep(Duration::from_secs(10));
            }
        }
    };
    if kind == "none" {
        say("no tmux or screen on this machine".into());
        return;
    }
    say(format!("{} session '{}'", kind, SESSION));
    // A recipe's window still open from before (orrery restarted while its
    // sudo waited): screen shows it, as tmux shows its current window.
    let open = if kind == "screen" {
        on(target, &format!("screen -S {} -Q windows 2>/dev/null | grep -o 'recipe-[0-9]*' | tail -n1", SESSION)).unwrap_or_default()
    } else {
        String::new()
    };
    let mut pending: VecDeque<Ask> = VecDeque::new();
    let mut size = (0u16, 0u16);
    // screen's window in front of the tile. A detached screen session has no
    // display, and a command sent to it without -p reaches no window at all
    // (28 October: typing into boburner's tile went nowhere), so every stuff
    // and every hardcopy names its window: the first ("0"), or the recipe's
    // (recipe-TIME) after Ask::Window until that window is gone.
    let mut win = if open.trim().is_empty() { String::from("0") } else { open.trim().to_string() };
    loop {
        // Everything typed since the last pass, in order, in one call.
        // Wait for something typed, or 900 ms for the next look at the
        // screen, whichever comes first: a key goes out at once, not on the
        // next second (28 October: "its delayed").
        if pending.is_empty() {
            if let Ok(a) = rx.recv_timeout(Duration::from_millis(900)) {
                pending.push_back(a);
                // The rest of a burst of typing, in the same call.
                thread::sleep(Duration::from_millis(30));
            }
        }
        while let Ok(a) = rx.try_recv() {
            pending.push_back(a);
        }
        let mut script = String::new();
        for a in pending.drain(..) {
            match (kind.as_str(), a) {
                (_, Ask::Size(c, r)) => {
                    if (c, r) != size {
                        size = (c, r);
                        if kind == "tmux" {
                            // Newer tmux sizes a window to its clients unless told
                            // it is sized by hand; orrery's tile is the size.
                            script.push_str(&format!("tmux set-option -t {s} window-size manual 2>/dev/null; tmux resize-window -t {s} -x {} -y {} 2>/dev/null; ", c, r, s = SESSION));
                        } else {
                            script.push_str(&format!("screen -S {s} -X width -w {} 2>/dev/null; screen -S {s} -X height -w {} 2>/dev/null; ", c, r, s = SESSION));
                        }
                    }
                }
                ("tmux", Ask::Window(n, c)) => script.push_str(&format!(
                    "tmux new-window -t {s}: -n {n}; tmux send-keys -t {s}:{n} -l -- {}; tmux send-keys -t {s}:{n} Enter; ",
                    q(&c),
                    s = SESSION,
                    n = n
                )),
                (_, Ask::Window(n, c)) => {
                    script.push_str(&format!(
                        "screen -S {s} -X screen -t {n}; screen -S {s} -p {n} -X stuff {}; ",
                        q(&format!("{}\n", c)),
                        s = SESSION,
                        n = n
                    ));
                    win = n;
                }
                ("tmux", Ask::Text(t)) => script.push_str(&format!("tmux send-keys -t {} -l -- {}; ", SESSION, q(&t))),
                ("tmux", Ask::Key(k, _)) => script.push_str(&format!("tmux send-keys -t {} {}; ", SESSION, k)),
                (_, Ask::Text(t)) => script.push_str(&format!("screen -S {} -p {} -X stuff {}; ", SESSION, q(&win), q(&t))),
                (_, Ask::Key(_, Some(c))) => {
                    script.push_str(&format!("screen -S {} -p {} -X stuff \"$(printf '\\{:03o}')\"; ", SESSION, q(&win), c as u32))
                }
                (_, Ask::Key(_, None)) => {}
            }
        }
        script.push_str(&if kind == "tmux" {
            format!("tmux capture-pane -p -t {}", SESSION)
        } else {
            // The named window. Gone only when screen's list no longer has
            // it (its shell exited): then the first, and a line to say so,
            // which the worker reads. A hardcopy of a window made a moment
            // ago may not be written yet, so it is tried again: taking that
            // for "gone" put the tile back on copal-tm (28 October).
            format!(
                "f=/tmp/orrery-hardcopy-$$; W={w}; \
                 if [ \"$W\" != 0 ] && ! screen -S {s} -Q windows 2>/dev/null | grep -q -- \"$W\"; then echo '@orrery-window-gone'; W=0; fi; \
                 for i in 1 2 3 4 5; do screen -S {s} -p \"$W\" -X hardcopy $f 2>/dev/null; [ -f $f ] && break; sleep 0.2; done; \
                 cat $f 2>/dev/null; rm -f $f",
                s = SESSION,
                w = q(&win)
            )
        });
        match on(target, &script) {
            Ok(text) => {
                let text = match text.strip_prefix("@orrery-window-gone\n") {
                    Some(rest) => {
                        win = "0".into();
                        rest.to_string()
                    }
                    None => text,
                };
                *lines.lock().unwrap_or_else(|e| e.into_inner()) = text.lines().map(|l| l.to_string()).collect();
                say(format!("{} session '{}'", kind, SESSION));
            }
            Err(e) => say(format!("lost: {}", e)),
        }
    }
}

/// Where the keyboard goes after a frame's presses: a left press on a tile's
/// screen focuses it; one in the body outside every tile clears it; one on a
/// title bar leaves it.
fn focus_after(events: &[((i32, i32), crate::surface::Button, bool)], screens: &[(usize, Rect, Rect)], body: Rect, now: Option<usize>) -> Option<usize> {
    let mut focus = now;
    for ((x, y), b, down) in events {
        if !*down || *b != crate::surface::Button::Left || !body.contains((*x, *y)) {
            continue;
        }
        if let Some((i, _, _)) = screens.iter().find(|(_, sc, _)| sc.contains((*x, *y))) {
            focus = Some(*i);
        } else if !screens.iter().any(|(_, _, whole)| whole.contains((*x, *y))) {
            focus = None;
        }
    }
    focus
}

/// Box-drawing and block characters as their ASCII look-alikes: the
/// bitmap faces carry Latin-1 only, and a program in a UTF-8 locale
/// (copal-tm on Ubuntu) draws its panels with these.
fn ascii_art(l: &str) -> String {
    l.chars()
        .map(|c| match c as u32 {
            0x2500 | 0x2501 | 0x2504 | 0x2505 | 0x2508 | 0x2509 | 0x254C | 0x254D | 0x2550 => '-',
            0x2502 | 0x2503 | 0x2506 | 0x2507 | 0x250A | 0x250B | 0x254E | 0x254F | 0x2551 => '|',
            0x250C..=0x254B | 0x2552..=0x256C | 0x256D..=0x2570 => '+',
            0x2571 => '/',
            0x2572 => '\\',
            0x2580..=0x259F => '#',
            0x25A0..=0x25AB | 0x2022 | 0x00B7 | 0x2219 => '.',
            0x2190..=0x21FF => '>',
            _ => c,
        })
        .collect()
}

/// A key the session should receive: tmux's name, and the byte for screen.
fn key_name(sym: Sym, ctrl: bool) -> Option<(&'static str, Option<char>)> {
    const CTRL: [&str; 26] = [
        "C-a", "C-b", "C-c", "C-d", "C-e", "C-f", "C-g", "C-h", "C-i", "C-j", "C-k", "C-l", "C-m", "C-n", "C-o", "C-p", "C-q",
        "C-r", "C-s", "C-t", "C-u", "C-v", "C-w", "C-x", "C-y", "C-z",
    ];
    match sym {
        Sym::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            let i = (c.to_ascii_lowercase() as u8 - b'a') as usize;
            Some((CTRL[i], Some((i as u8 + 1) as char)))
        }
        Sym::Return => Some(("Enter", Some('\r'))),
        Sym::Backspace => Some(("BSpace", Some('\u{7f}'))),
        Sym::Tab => Some(("Tab", Some('\t'))),
        Sym::Escape => Some(("Escape", Some('\u{1b}'))),
        Sym::Up => Some(("Up", None)),
        Sym::Down => Some(("Down", None)),
        Sym::Left => Some(("Left", None)),
        Sym::Right => Some(("Right", None)),
        Sym::Home => Some(("Home", None)),
        Sym::End => Some(("End", None)),
        Sym::PageUp => Some(("PageUp", None)),
        Sym::PageDown => Some(("PageDown", None)),
        Sym::Delete => Some(("DC", None)),
        _ => None,
    }
}

/// Draw the grid (or one tile enlarged) and pass the keyboard to the focused
/// tile. Returns a view to move to, if one was asked for.
pub fn draw(ui: &mut Ui, s: &mut Sessions) -> Option<nav::View> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let t = ui.t;
    ui.panel(rect(0, 0, w, h), t.base);
    let (header, rest) = rect(0, 0, w, h).cut_top(HEADER_H);
    ui.panel(header, t.panel);
    ui.hrule(header.x, header.bottom() - 1, header.w);
    let ty = header.y + (HEADER_H - F10X20.height as i32) / 2;
    ui.label(PAD, ty, "orrery", &F10X20, t.ink);
    let nav_at = rect(PAD + F10X20.measure("orrery") as i32 + 14, header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Sessions);
    if let Some(v) = nav::chord(ui) {
        go = Some(v);
    }
    let sy = header.y + (HEADER_H - F8X13.height as i32) / 2;
    let hint = match s.focus {
        Some(i) => format!("typing into {} -- click outside the tiles to stop", s.tiles[i].name),
        None => "click a tile to type into it; each is a tmux session that outlives orrery".to_string(),
    };
    nav::header_right(ui, header, PAD, sy, &hint);

    let body = rest.inset(6);
    let n = s.tiles.len().max(1) as i32;
    let rects: Vec<(usize, Rect)> = match s.big {
        Some(i) if i < s.tiles.len() => vec![(i, body)],
        _ => {
            let cols = if n <= 2 { n } else if n <= 4 { 2 } else { 3 };
            let rows = (n + cols - 1) / cols;
            let (tw, th) = ((body.w - (cols - 1) * 6) / cols, (body.h - (rows - 1) * 6) / rows);
            (0..s.tiles.len())
                .map(|i| {
                    let (c, r) = (i as i32 % cols, i as i32 / cols);
                    (i, rect(body.x + c * (tw + 6), body.y + r * (th + 6), tw, th))
                })
                .collect()
        }
    };

    let mut clicked_any = false;
    let mut toggle_big = None;
    // (tile, its screen, the whole tile), for where a press lands.
    let mut screens: Vec<(usize, Rect, Rect)> = Vec::new();
    for (i, r) in &rects {
        let (i, r) = (*i, *r);
        let tile = &mut s.tiles[i];
        let focused = s.focus == Some(i);
        ui.panel(r, t.tile);
        ui.outline(r, if focused { t.accent } else { t.line });
        let (title, screen) = r.cut_top(TITLE_H);
        ui.panel(title, t.panel);
        let status = tile.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let label = if tile.target == "-" { "this Mac".to_string() } else { tile.name.clone() };
        let lx = ui.label_in(title.x + 6, title.y + 3, title.w / 3, &label, &F8X13, if focused { t.ink } else { t.dim });
        let bw = 70;
        let br = rect(title.right() - bw - 2, title.y + 1, bw, TITLE_H - 2);
        if ui.button(br, if s.big == Some(i) { "Grid" } else { "Enlarge" }, true) {
            toggle_big = Some(i);
        }
        let sx = lx.max(title.x + 6) + 16;
        ui.label_in(sx, title.y + 3, br.x - sx - 8, &status, &F8X13, t.dim);

        // The characters the tile can show; the session is told, so a program
        // in it lays itself out to fit.
        let inner = screen.inset(4);
        let cols = (inner.w / F6X10.width as i32).max(20) as u16;
        let rows = (inner.h / F6X10.height as i32).max(5) as u16;
        if (cols, rows) != tile.size {
            tile.size = (cols, rows);
            let _ = tile.tx.send(Ask::Size(cols, rows));
        }
        let mut lines = tile.lines.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // The multiplexer gives the whole pane, blank rows under the prompt
        // included: drop those, then show the last rows that fit, so a
        // prompt on the first line is never cut off with only blanks shown.
        while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(rows as usize);
        for (k, l) in lines[start..].iter().enumerate() {
            ui.label_in(inner.x, inner.y + k as i32 * F6X10.height as i32, inner.w, &ascii_art(l), &F6X10, t.ink);
        }
        screens.push((i, screen, r));
    }
    if let Some(i) = toggle_big {
        s.big = if s.big == Some(i) { None } else { Some(i) };
        s.focus = Some(i);
        clicked_any = true;
    }
    // Focus follows the press: on a tile's screen, that tile; in the body
    // outside every tile, none; on a title bar, as it was (its button has
    // its say above). Two press-and-release areas, one inside the other,
    // made the outer one win, so a click on a tile stopped typing into it
    // (28 October: "I can't get it to receive enter key or any thing").
    if !clicked_any {
        s.focus = focus_after(&ui.f.button_events(), &screens, body, s.focus);
    }

    // The keyboard goes to the focused tile, except the view chords.
    if let Some(i) = s.focus {
        let tile = &s.tiles[i];
        let mut typed = false;
        // In the order typed: letters and Return in one frame stay letters
        // first (a password's Return before its last letters is a wrong
        // password).
        for t in ui.f.typed.clone() {
            match t {
                crate::ui::Typed::Key(sym, mods) => {
                    if mods.logo {
                        continue; // Command-digit and the like belong to orrery
                    }
                    if let Some((k, b)) = key_name(sym, mods.ctrl) {
                        let _ = tile.tx.send(Ask::Key(k, b));
                        typed = true;
                    }
                }
                crate::ui::Typed::Text(text) => {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        let _ = tile.tx.send(Ask::Text(text));
                        typed = true;
                    }
                }
            }
        }
        let _ = typed;
    }
    go
}

#[cfg(test)]
mod tests {
    use crate::surface::Button;

    #[test]
    fn a_press_on_a_tile_focuses_it_and_one_between_tiles_does_not() {
        let body = rect(0, 40, 600, 400);
        // Two tiles: title bar 18 high, then the screen.
        let screens = vec![(0, rect(0, 58, 290, 382), rect(0, 40, 290, 400)), (1, rect(310, 58, 290, 382), rect(310, 40, 290, 400))];
        let press = |x, y| vec![((x, y), Button::Left, true), ((x, y), Button::Left, false)];
        assert_eq!(focus_after(&press(100, 200), &screens, body, None), Some(0), "on tile 0's screen");
        assert_eq!(focus_after(&press(400, 200), &screens, body, Some(0)), Some(1), "on tile 1's screen");
        assert_eq!(focus_after(&press(300, 200), &screens, body, Some(1)), None, "between the tiles");
        assert_eq!(focus_after(&press(100, 45), &screens, body, Some(1)), Some(1), "a title bar leaves it");
        assert_eq!(focus_after(&press(100, 200)[1..], &screens, body, None), None, "a release alone is no press");
    }

    use super::*;

    #[test]
    fn keys_are_named_for_tmux_and_screen() {
        assert_eq!(key_name(Sym::Char('c'), true), Some(("C-c", Some('\u{3}'))));
        assert_eq!(key_name(Sym::Return, false), Some(("Enter", Some('\r'))));
        assert_eq!(key_name(Sym::Char('c'), false), None, "plain text travels as text");
    }

    #[test]
    fn box_drawing_becomes_ascii() {
        assert_eq!(ascii_art("╭──╮"), "+--+");
        assert_eq!(ascii_art("│CPU ▪▪ 99.9%│"), "|CPU .. 99.9%|");
    }

    #[test]
    fn quoting_survives_a_single_quote() {
        assert_eq!(q("it's"), "'it'\\''s'");
    }
}
