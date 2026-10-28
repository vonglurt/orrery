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
            let lines = Arc::new(Mutex::new(Vec::new()));
            let status = Arc::new(Mutex::new("connecting".to_string()));
            let (tx, rx) = channel();
            let (l2, s2, t2) = (Arc::clone(&lines), Arc::clone(&status), target.clone());
            thread::spawn(move || worker(&t2, rx, l2, s2));
            s.tiles.push(Tile { name, target, lines, status, tx, size: (0, 0) });
        }
        s
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
    let mut pending: VecDeque<Ask> = VecDeque::new();
    let mut size = (0u16, 0u16);
    loop {
        // Everything typed since the last pass, in order, in one call.
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
                            script.push_str(&format!("tmux resize-window -t {} -x {} -y {} 2>/dev/null; ", SESSION, c, r));
                        }
                    }
                }
                ("tmux", Ask::Text(t)) => script.push_str(&format!("tmux send-keys -t {} -l -- {}; ", SESSION, q(&t))),
                ("tmux", Ask::Key(k, _)) => script.push_str(&format!("tmux send-keys -t {} {}; ", SESSION, k)),
                (_, Ask::Text(t)) => script.push_str(&format!("screen -S {} -X stuff {}; ", SESSION, q(&t))),
                (_, Ask::Key(_, Some(c))) => script.push_str(&format!("screen -S {} -X stuff \"$(printf '\\{:03o}')\"; ", SESSION, c as u32)),
                (_, Ask::Key(_, None)) => {}
            }
        }
        script.push_str(&if kind == "tmux" {
            format!("tmux capture-pane -p -t {}", SESSION)
        } else {
            format!("f=/tmp/orrery-hardcopy-$$; screen -S {} -X hardcopy $f && cat $f; rm -f $f", SESSION)
        });
        match on(target, &script) {
            Ok(text) => {
                *lines.lock().unwrap_or_else(|e| e.into_inner()) = text.lines().map(|l| l.to_string()).collect();
                say(format!("{} session '{}'", kind, SESSION));
            }
            Err(e) => say(format!("lost: {}", e)),
        }
        thread::sleep(Duration::from_millis(900));
    }
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
    ui.label_right(header.right() - PAD, sy, &hint, &F8X13, t.dim);

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
    for (i, r) in &rects {
        let (i, r) = (*i, *r);
        let tile = &mut s.tiles[i];
        let focused = s.focus == Some(i);
        ui.panel(r, t.tile);
        ui.outline(r, if focused { t.accent } else { t.line });
        let (title, screen) = r.cut_top(TITLE_H);
        ui.panel(title, t.panel);
        let status = tile.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let label = format!("{}  {}", tile.name, if tile.target == "-" { "this Mac" } else { &tile.target });
        ui.label_in(title.x + 6, title.y + 3, title.w / 2, &label, &F8X13, if focused { t.ink } else { t.dim });
        let bw = 70;
        let br = rect(title.right() - bw - 2, title.y + 1, bw, TITLE_H - 2);
        if ui.button(br, if s.big == Some(i) { "Grid" } else { "Enlarge" }, true) {
            toggle_big = Some(i);
        }
        ui.label_in(title.x + title.w / 2, title.y + 3, title.w / 2 - bw - 10, &status, &F8X13, t.dim);

        // The characters the tile can show; the session is told, so a program
        // in it lays itself out to fit.
        let inner = screen.inset(4);
        let cols = (inner.w / F6X10.width as i32).max(20) as u16;
        let rows = (inner.h / F6X10.height as i32).max(5) as u16;
        if (cols, rows) != tile.size {
            tile.size = (cols, rows);
            let _ = tile.tx.send(Ask::Size(cols, rows));
        }
        let lines = tile.lines.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // The bottom of the screen is where the prompt is: show the last rows.
        let start = lines.len().saturating_sub(rows as usize);
        for (k, l) in lines[start..].iter().enumerate() {
            ui.label_in(inner.x, inner.y + k as i32 * F6X10.height as i32, inner.w, l, &F6X10, t.ink);
        }
        if ui.button_hit(screen) {
            s.focus = Some(i);
            clicked_any = true;
        }
    }
    if let Some(i) = toggle_big {
        s.big = if s.big == Some(i) { None } else { Some(i) };
        s.focus = Some(i);
        clicked_any = true;
    }
    // A click on the body outside every tile stops typing.
    if !clicked_any && ui.button_hit(body) {
        s.focus = None;
    }

    // The keyboard goes to the focused tile, except the view chords.
    if let Some(i) = s.focus {
        let tile = &s.tiles[i];
        let mut typed = false;
        for (sym, mods) in ui.f.keys.clone() {
            if mods.logo {
                continue; // Command-digit and the like belong to orrery
            }
            if let Some((k, b)) = key_name(sym, mods.ctrl) {
                let _ = tile.tx.send(Ask::Key(k, b));
                typed = true;
            }
        }
        let text: String = ui.f.text.chars().filter(|c| !c.is_control()).collect();
        if !text.is_empty() {
            let _ = tile.tx.send(Ask::Text(text));
            typed = true;
        }
        let _ = typed;
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_for_tmux_and_screen() {
        assert_eq!(key_name(Sym::Char('c'), true), Some(("C-c", Some('\u{3}'))));
        assert_eq!(key_name(Sym::Return, false), Some(("Enter", Some('\r'))));
        assert_eq!(key_name(Sym::Char('c'), false), None, "plain text travels as text");
    }

    #[test]
    fn quoting_survives_a_single_quote() {
        assert_eq!(q("it's"), "'it'\\''s'");
    }
}
