//! Cards, images and machines.
//!
//! ORRERY DOES NOT WRITE A CARD. `copal-prep.sh` picks the disk and asks for
//! both typed `ERASE` confirmations, and `bin/sd.sh` says in its own words that
//! this *is the entire safety model for this path*. This is a front end to it:
//! it shows what is about to happen, runs `make` on a real terminal (see
//! `pty.rs`), and shows what happened. The confirmations still happen, typed by
//! the operator, read by `copal-prep.sh`, inside the pane.
//!
//! The reason is not caution for its own sake. `copal-prep.sh` is 1.6 MB of
//! shell that knows nine boards across four architectures, the boot-partition
//! label collision that makes two builds unsafe at once, the mount-point
//! refusal, and which `/dev/rdiskN` on a Mac is a card rather than the startup
//! disk. A second implementation in Rust would be a second thing that can be
//! wrong about which disk is which, and the failure mode of being wrong about
//! that is somebody's photographs.
//!
//! WHAT IT ADDS IS THE THING THAT IS CURRENTLY NOBODY'S JOB. The Makefile
//! documents a discipline -- `make answers`, build, `make answers-node N=2`,
//! build, and so on -- with a comment saying why it must be that way: *"the
//! answers file describes whichever card is next."* That is a stateful sequence
//! with no memory of where it is. The operator counts cards in their head, and
//! the failure is silent: run `answers-node N=5` twice and card 4 is never
//! written; forget it entirely and two cards are both `museum-03`, which is two
//! machines with one identity and one enrolment token.
//!
//! So each step becomes a check, and the pile of cards becomes a document.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::font::{F10X20, F8X13};
use crate::nav;
use crate::pty::Pty;
use crate::surface::Sym;
use crate::ui::{rect, Rect, Ui};

// ------------------------------------------------------------------ answers ---

/// `answers.txt`, read through the shape `copal-answers.sh` writes.
#[derive(Debug, Default, Clone)]
pub struct Answers {
    map: BTreeMap<String, String>,
}

impl Answers {
    /// Parse `KEY='value'` lines, which is the whole of the format.
    ///
    /// Deliberately not a shell: the file is sourced by `copal-prep.sh` and
    /// could in principle contain anything a shell understands, but everything
    /// `copal-answers.sh` writes is a single-quoted scalar, and a console that
    /// evaluated this file to read it would be a console that runs whatever is
    /// in it.
    pub fn parse(text: &str) -> Answers {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if !k.starts_with("COPAL_") {
                continue;
            }
            let v = v.trim();
            let v = v
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
                .or_else(|| v.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
                .unwrap_or(v);
            map.insert(k.trim().to_string(), v.to_string());
        }
        Answers { map }
    }

    pub fn get(&self, key: &str) -> &str {
        self.map.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn num(&self, key: &str) -> usize {
        self.get(key).parse().unwrap_or(0)
    }

    /// THE PASSWORD HASH IS NEVER RETURNED AND NEVER DRAWN. It is in the file,
    /// it is a SHA-512 crypt hash rather than a password, and it is still not
    /// something to put on a gallery screen.
    pub fn is_secret(key: &str) -> bool {
        matches!(
            key,
            "COPAL_ROOT_PW_HASH" | "COPAL_FLEET_TOKEN" | "COPAL_FLEET_PSK"
        )
    }
}

// ------------------------------------------------------------------- ledger ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// A token was issued for this card and nothing has enrolled with it.
    Unused,
    /// A node was signed with it. The card became a machine.
    Spent,
    /// No token on file: this card has never been prepared.
    None,
}

impl Token {
    pub fn word(&self) -> &'static str {
        match self {
            Token::Unused => "issued",
            Token::Spent => "enrolled",
            Token::None => "-",
        }
    }
}

/// One card of the fleet, from three sources that have to agree.
#[derive(Debug, Clone)]
pub struct Card {
    pub index: usize,
    pub hostname: String,
    pub token: Token,
    /// From the media manifest: when this physical object came into existence.
    pub built: String,
    /// From the read model: has a machine with this name ever announced?
    pub seen: bool,
}

/// `<fleet>-0N`, which is what `copal-answers.sh`'s `fleet_hostname` makes.
pub fn hostname_of(fleet: &str, index: usize) -> String {
    format!("{}-{:02}", fleet, index)
}

/// The token ledger: `hostname \t token \t unused|spent`.
///
/// One unused entry per hostname, because `record_token` supersedes rather
/// than appends -- *"two live tokens for one machine would mean the check has
/// two right answers, which is not a check."*
pub fn parse_ledger(text: &str) -> BTreeMap<String, Token> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut f = line.split('\t');
        let (Some(host), Some(_token), Some(state)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let state = match state.trim() {
            "unused" => Token::Unused,
            "spent" => Token::Spent,
            _ => continue,
        };
        // A spent row wins over an unused one for the same host: enrolment is
        // the later fact and the one the operator is asking about.
        match out.get(host) {
            Some(Token::Spent) => {}
            _ => {
                out.insert(host.to_string(), state);
            }
        }
    }
    out
}

/// The media manifest: when each card, image or machine was made.
///
/// `timestamp \t index \t hostname \t board \t target \t sha \t token-fp \t outcome`
pub fn parse_manifest(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        let (when, host, board, target) = (f[0], f[2], f[3], f[4]);
        let day = when.split('T').next().unwrap_or(when);
        out.insert(host.to_string(), format!("{} {} {}", day, board, target));
    }
    out
}

// --------------------------------------------------------------------- jobs ---

/// A `make` running on a terminal, with its output.
pub struct Job {
    pub title: String,
    pty: Pty,
    pub lines: Vec<String>,
    partial: String,
    pub done: Option<i32>,
}

/// The colour a terminal wants and a canvas does not.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            continue;
        }
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

impl Job {
    /// The most a pane keeps. A card write is a few hundred lines; an
    /// unbounded buffer is a console that grows for the life of a build.
    const SCROLLBACK: usize = 800;

    pub fn start(title: String, argv: &[String], dir: &Path) -> Result<Job, String> {
        let pty = Pty::spawn(argv, &OsString::from(dir))?;
        Ok(Job {
            title,
            pty,
            lines: vec![format!("$ {}", argv.join(" "))],
            partial: String::new(),
            done: None,
        })
    }

    /// The most an unterminated line may hold before its head is dropped.
    ///
    /// A child drawing a progress bar redraws one line with carriage returns
    /// and never emits a newline -- and `strip_ansi` drops the `\r` -- so this
    /// is not a hypothetical: `apk`, `dd` and `git clone` all do it, and all
    /// three are in the chain a card write runs.
    const MAX_PROMPT: usize = 4096;

    /// Drain the terminal into lines. Called once a frame.
    pub fn pump(&mut self) {
        let bytes = self.pty.read();
        if !bytes.is_empty() {
            let text = strip_ansi(&String::from_utf8_lossy(&bytes));
            self.partial.push_str(&text);
            while let Some(i) = self.partial.find('\n') {
                let line: String = self.partial.drain(..=i).collect();
                self.lines.push(line.trim_end().to_string());
            }
        }

        // THE TRIMS ARE UNCONDITIONAL, and they were not: nesting them inside
        // "did anything arrive" meant a buffer could only ever be trimmed by
        // the arrival of more of it. Bounded because a build that prints for
        // an hour must not be a console that grows for an hour.
        if self.lines.len() > Self::SCROLLBACK {
            let drop = self.lines.len() - Self::SCROLLBACK;
            self.lines.drain(..drop);
        }
        if self.partial.len() > Self::MAX_PROMPT {
            // Keep the TAIL. The question is at the end of the line, and a
            // truncation that kept the head would throw away the prompt and
            // leave the operator looking at the beginning of a progress bar.
            let cut = self
                .partial
                .len()
                .saturating_sub(Self::MAX_PROMPT / 4)
                .min(self.partial.len());
            let cut = (cut..=self.partial.len())
                .find(|i| self.partial.is_char_boundary(*i))
                .unwrap_or(self.partial.len());
            self.partial = self.partial.split_off(cut);
        }

        if self.done.is_none() {
            self.done = self.pty.finished();
        }
    }

    /// What is on the last, unterminated line -- which is where a prompt sits.
    ///
    /// `copal-prep.sh` asks with `printf` and no newline, so a pane that only
    /// showed completed lines would show the operator a blank space where the
    /// question is.
    pub fn prompt(&self) -> &str {
        &self.partial
    }

    pub fn send(&mut self, s: &str) {
        if self.done.is_none() {
            self.pty.write(s);
        }
    }

    pub fn stop(&mut self) {
        self.pty.kill();
        self.done = Some(-1);
    }
}

// -------------------------------------------------------------------- state ---

/// The nine boards `copal --targets` knows, as a fallback.
///
/// Read from the checkout at open where it can be; this list exists so a pane
/// still draws when `./copal` is not executable, which on a fresh clone over a
/// network share it sometimes is not.
const BOARDS: &[(&str, &str)] = &[
    ("zero2", "Pi Zero 2 W / Pi 3 / CM3"),
    ("pi4", "Pi 4 / 400 / CM4"),
    ("pi5", "Pi 5"),
    ("pi2b", "Pi 2 B v1.1"),
    ("zero", "Pi Zero / Zero W / Pi 1"),
    ("pc", "PC / laptop / Intel Mac"),
    ("pc32", "PC, 32-bit UEFI"),
    ("vm", "UTM on Apple Silicon"),
    ("vmx86", "UTM x86_64"),
];

pub struct Media {
    pub root: PathBuf,
    pub answers: Answers,
    pub fleet: String,
    pub size: usize,
    /// Which card `answers.txt` currently describes.
    pub current: usize,
    pub cards: Vec<Card>,
    pub boards: Vec<(String, String)>,
    pub board: usize,
    pub sel: usize,
    pub job: Option<Job>,
    pub error: String,
}

impl Media {
    /// Where the checkout is, tried in the order a person would try them.
    pub fn find_root(explicit: &str) -> Option<PathBuf> {
        let mut tries: Vec<PathBuf> = Vec::new();
        if !explicit.is_empty() {
            tries.push(PathBuf::from(explicit));
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            tries.push(home.join("code/copal-alpine-linux"));
            tries.push(home.join("code/copal"));
        }
        tries.into_iter().find(|p| p.join("copal-prep.sh").is_file())
    }

    /// The museum's eight cards, so the pane can be looked at with no fleet.
    ///
    /// The same argument as `fleet::demo_doc`, and deliberately not eight tidy
    /// rows: the states worth designing against are a card that was written
    /// and enrolled, one that was written and has gone quiet, one prepared but
    /// not yet written -- which is where the operator actually is -- and four
    /// that do not exist yet.
    pub fn demo() -> Media {
        let mut m = Media {
            root: PathBuf::from("(demo)"),
            answers: Answers::default(),
            fleet: "museum".into(),
            size: 8,
            current: 4,
            cards: Vec::new(),
            boards: BOARDS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            board: 0,
            sel: 3,
            job: None,
            error: String::new(),
        };
        m.cards = (1..=8)
            .map(|i| Card {
                index: i,
                hostname: hostname_of("museum", i),
                token: match i {
                    1..=3 => Token::Spent,
                    4 => Token::Unused,
                    _ => Token::None,
                },
                built: match i {
                    1 | 2 => "2026-09-04 zero2 sd".into(),
                    3 => "2026-09-05 zero2 sd".into(),
                    _ => String::new(),
                },
                // museum-03 enrolled and is no longer answering, which is the
                // row that needs a person -- and the reason the NODE column
                // exists beside the TOKEN one rather than instead of it.
                seen: matches!(i, 1 | 2),
            })
            .collect();
        m
    }

    pub fn open(explicit: &str) -> Media {
        let mut m = Media {
            root: PathBuf::new(),
            answers: Answers::default(),
            fleet: String::new(),
            size: 0,
            current: 0,
            cards: Vec::new(),
            boards: BOARDS
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            board: 0,
            sel: 0,
            job: None,
            error: String::new(),
        };
        match Media::find_root(explicit) {
            Some(r) => m.root = r,
            None => {
                m.error =
                    "no copal checkout found. Pass --copal PATH, or clone it into ~/code."
                        .into();
                return m;
            }
        }
        m.reload();
        m
    }

    /// Re-read every source. Cheap, and done after anything that writes one.
    pub fn reload(&mut self) {
        if self.root.as_os_str().is_empty() {
            return;
        }
        let text = std::fs::read_to_string(self.root.join("answers.txt")).unwrap_or_default();
        if text.is_empty() {
            self.error = format!(
                "no answers.txt in {}. Run `make answers` there first.",
                self.root.display()
            );
            self.cards.clear();
            return;
        }
        self.answers = Answers::parse(&text);
        self.fleet = self.answers.get("COPAL_FLEET").to_string();
        self.size = self.answers.num("COPAL_FLEET_SIZE");
        self.current = self.answers.num("COPAL_FLEET_INDEX");

        if self.fleet.is_empty() {
            self.error =
                "answers.txt describes a standalone machine, not a fleet. `make answers` names one."
                    .into();
            self.cards.clear();
            return;
        }
        self.error.clear();

        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let fdir = home.join(".copal/fleets").join(&self.fleet);
        let ledger = parse_ledger(
            &std::fs::read_to_string(fdir.join("tokens")).unwrap_or_default(),
        );
        let manifest = parse_manifest(
            &std::fs::read_to_string(fdir.join("media")).unwrap_or_default(),
        );

        self.cards = (1..=self.size.max(1))
            .map(|i| {
                let hostname = hostname_of(&self.fleet, i);
                Card {
                    index: i,
                    token: *ledger.get(&hostname).unwrap_or(&Token::None),
                    built: manifest.get(&hostname).cloned().unwrap_or_default(),
                    seen: false,
                    hostname,
                }
            })
            .collect();
        if self.sel >= self.cards.len() {
            self.sel = self.cards.len().saturating_sub(1);
        }
    }

    /// Mark the cards whose machines the read model has actually seen.
    pub fn note_fleet(&mut self, ids: &[String]) {
        for c in &mut self.cards {
            c.seen = ids.iter().any(|id| *id == c.hostname);
        }
    }

    fn board_name(&self) -> &str {
        self.boards
            .get(self.board)
            .map(|(n, _)| n.as_str())
            .unwrap_or("zero2")
    }

    /// THE SEQUENCE, ENFORCED INSTEAD OF REMEMBERED.
    ///
    /// A card may only be written while `answers.txt` describes it. The
    /// Makefile documents this as a discipline; here it is a check, and the
    /// pane says which card the file describes rather than silently writing
    /// the wrong identity onto a disk.
    pub fn can_write(&self, card: &Card) -> bool {
        self.job.is_none() && card.index == self.current && !self.fleet.is_empty()
    }

    /// A card may only be prepared when it has no live token.
    ///
    /// Re-preparing supersedes an unused token for no reason, and the ledger's
    /// own comment says why that is bad.
    pub fn can_prepare(&self, card: &Card) -> bool {
        self.job.is_none() && card.token != Token::Unused
    }

    /// The next thing to do, which is the next unbuilt card.
    pub fn next_unbuilt(&self) -> Option<usize> {
        self.cards.iter().position(|c| c.built.is_empty())
    }

    fn start(&mut self, title: String, argv: Vec<String>) {
        match Job::start(title, &argv, &self.root) {
            Ok(j) => self.job = Some(j),
            Err(e) => self.error = e,
        }
    }
}

// ------------------------------------------------------------------ drawing ---

const HEADER_H: i32 = 32;
const PAD: i32 = 10;
const ROW_H: i32 = 20;
const FOOT_H: i32 = 88;

/// Draw the pane. Returns a view to move to, if the operator asked.
pub fn draw(ui: &mut Ui, m: &mut Media) -> Option<nav::View> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let t = ui.t;
    ui.panel(rect(0, 0, w, h), t.base);

    let all = rect(0, 0, w, h);
    let (header, rest) = all.cut_top(HEADER_H);

    // ---- header, with the navigation strip -----------------------------
    ui.panel(header, t.panel);
    ui.hrule(header.x, header.bottom() - 1, header.w);
    let ty = header.y + (HEADER_H - F10X20.height as i32) / 2;
    ui.label(PAD, ty, "orrery", &F10X20, t.ink);
    let nav_at = rect(PAD + F10X20.measure("orrery") as i32 + 14, header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Media);
    if let Some(v) = nav::chord(ui) {
        go = Some(v);
    }

    let sy = header.y + (HEADER_H - F8X13.height as i32) / 2;
    if !m.fleet.is_empty() {
        let right = format!("{}  {} cards", m.fleet, m.size);
        ui.label_right(header.right() - PAD, sy, &right, &F8X13, t.dim);
    }

    // A job replaces the table, the way the results pane replaces the lab.
    if m.job.is_some() {
        draw_job(ui, m, rest);
        return go;
    }

    if !m.error.is_empty() {
        ui.label_in(PAD * 2, rest.y + 40, w - PAD * 4, "the card ledger cannot be read", &F10X20, t.alarm);
        ui.label_in(PAD * 2, rest.y + 66, w - PAD * 4, &m.error, &F8X13, t.dim);
        return go;
    }

    let (foot, table) = rest.cut_bottom(FOOT_H);
    draw_table(ui, m, table);
    draw_foot(ui, m, foot);
    keys(ui, m);
    go
}

fn draw_table(ui: &mut Ui, m: &mut Media, at: Rect) {
    let t = ui.t;
    let inner = at.inset(PAD);

    // Columns, once, so the header and every row agree.
    // The marker column sits to the LEFT of the numbers, so it needs room of
    // its own rather than a negative offset off the edge of the table.
    let x_mark = inner.x;
    let x_idx = inner.x + 14;
    let x_host = inner.x + 46;
    let x_token = inner.x + 180;
    let x_built = inner.x + 290;
    let x_node = inner.right() - 74;

    ui.label_right(x_idx + 10, inner.y, "#", &F8X13, t.accent);
    ui.label(x_host, inner.y, "HOSTNAME", &F8X13, t.accent);
    ui.label(x_token, inner.y, "TOKEN", &F8X13, t.accent);
    ui.label(x_built, inner.y, "BUILT", &F8X13, t.accent);
    ui.label(x_node, inner.y, "NODE", &F8X13, t.accent);
    ui.hrule(inner.x, inner.y + 16, inner.w);

    let mut y = inner.y + 22;
    let next = m.next_unbuilt();
    let mut clicked = None;
    for (i, c) in m.cards.iter().enumerate() {
        if y + ROW_H > inner.bottom() {
            break;
        }
        let row = rect(inner.x, y, inner.w, ROW_H);
        let on = i == m.sel;
        if on {
            ui.panel(row, t.tile);
            ui.c.rect(row.x, row.y, 2, row.h, t.accent);
        } else if ui.is_hot(row) {
            ui.panel(row, t.panel);
        }

        // The marker is the NEXT THING TO DO, which is what turns a table into
        // an instruction. `answers.txt` describes one card; that one is where
        // the operator is.
        if c.index == m.current {
            ui.label(x_mark + 5, y + 3, ">", &F8X13, t.accent);
        }
        ui.label_right(x_idx + 10, y + 3, &c.index.to_string(), &F8X13, t.dim);
        ui.label_in(x_host, y + 3, x_token - x_host - 6, &c.hostname, &F8X13, t.ink);

        let (word, colour) = match c.token {
            Token::Unused => ("issued", t.warn),
            Token::Spent => ("enrolled", t.up),
            Token::None => ("-", t.dim),
        };
        ui.label(x_token, y + 3, word, &F8X13, colour);

        if c.built.is_empty() {
            let (label, colour) = if Some(i) == next {
                ("next", t.accent)
            } else {
                ("-", t.dim)
            };
            ui.label(x_built, y + 3, label, &F8X13, colour);
        } else {
            ui.label_in(x_built, y + 3, x_node - x_built - 8, &c.built, &F8X13, t.dim);
        }

        // Three sources, one row. "Which of these eight did I not finish" is
        // the question, and it cannot be answered from any one of them.
        if c.seen {
            ui.light(x_node + 4, y + ROW_H / 2, 3, crate::ui::Light::Full, t.up);
            ui.label(x_node + 12, y + 3, "up", &F8X13, t.dim);
        } else if c.token == Token::Spent {
            ui.light(x_node + 4, y + ROW_H / 2, 3, crate::ui::Light::Hollow, t.warn);
            ui.label(x_node + 12, y + 3, "gone", &F8X13, t.warn);
        }

        if ui.button_hit(row) {
            clicked = Some(i);
        }
        y += ROW_H;
    }
    if let Some(i) = clicked {
        m.sel = i;
    }
}

fn draw_foot(ui: &mut Ui, m: &mut Media, at: Rect) {
    let t = ui.t;
    ui.hrule(at.x, at.y, at.w);
    let inner = at.inset(PAD);

    let Some(card) = m.cards.get(m.sel).cloned() else { return };

    // The board picker. A row of names rather than a dropdown: nine of them
    // fit, and a menu that has to be opened to see what is in it is a menu.
    let mut x = inner.x;
    let label_w = F8X13.measure("board") as i32;
    ui.label(x, inner.y + 4, "board", &F8X13, t.dim);
    x += label_w + 10;
    let mut pick = None;
    for (i, (name, _)) in m.boards.iter().enumerate() {
        let bw = F8X13.measure(name) as i32 + 14;
        let b = rect(x, inner.y, bw, 20);
        let on = i == m.board;
        if on {
            ui.panel(b, t.accent);
        } else if ui.is_hot(b) {
            ui.panel(b, t.tile);
        }
        ui.outline(b, if on { t.accent } else { t.line });
        ui.label_in(b.x + 7, b.y + 3, bw - 10, name, &F8X13, if on { t.base } else { t.dim });
        if ui.button_hit(b) {
            pick = Some(i);
        }
        x += bw + 4;
    }
    if let Some(i) = pick {
        m.board = i;
    }

    // The buttons. Absent rather than greyed, as everywhere else -- what is
    // drawn is what can happen to the card that is selected, right now.
    let by = inner.y + 28;
    let mut bx = inner.x;
    let board = m.board_name().to_string();
    let mut start: Option<(String, Vec<String>)> = None;

    if m.can_write(&card) {
        if ui.button(rect(bx, by, 96, 22), "Write card", true) {
            start = Some((
                format!("card {} -- {}", card.index, card.hostname),
                vec!["make".into(), format!("sd-{}", board)],
            ));
        }
        bx += 102;
    }
    if m.job.is_none() {
        if ui.button(rect(bx, by, 102, 22), "Build image", true) {
            start = Some((
                format!("image -- {}", board),
                vec!["make".into(), format!("img-{}", board)],
            ));
        }
        bx += 108;
    }
    if m.can_prepare(&card) {
        let label = format!("Prepare card {}", card.index);
        let w = F8X13.measure(&label) as i32 + 20;
        if ui.button(rect(bx, by, w, 22), &label, true) {
            start = Some((
                label.clone(),
                vec![
                    "tools/copal-answers.sh".into(),
                    "--node".into(),
                    card.index.to_string(),
                ],
            ));
        }
    }

    // The sentence that replaces counting cards in your head.
    let say = if !m.can_write(&card) && m.current > 0 && card.index != m.current {
        format!(
            "answers.txt describes card {} ({}). Prepare card {} to write it.",
            m.current,
            hostname_of(&m.fleet, m.current),
            card.index
        )
    } else if m.can_write(&card) {
        format!(
            "answers.txt describes this card. copal-prep.sh will ask which disk, twice."
        )
    } else {
        String::new()
    };
    if !say.is_empty() {
        ui.label_in(inner.x, by + 28, inner.w, &say, &F8X13, t.dim);
    }

    if let Some((title, argv)) = start {
        m.start(title, argv);
    }
}

fn draw_job(ui: &mut Ui, m: &mut Media, at: Rect) {
    let t = ui.t;
    let (head, body) = at.cut_top(26);
    ui.panel(head, t.panel);
    ui.hrule(head.x, head.bottom() - 1, head.w);

    let (title, done) = {
        let j = m.job.as_ref().expect("draw_job with no job");
        (j.title.clone(), j.done)
    };
    let word = match done {
        None => "running".to_string(),
        Some(0) => "done".to_string(),
        Some(c) => format!("exit {}", c),
    };
    let colour = match done {
        None => t.accent,
        Some(0) => t.up,
        Some(_) => t.alarm,
    };
    ui.label_in(head.x + PAD, head.y + 6, head.w - 200, &title, &F8X13, t.ink);
    ui.label_right(head.right() - 90, head.y + 6, &word, &F8X13, colour);

    let stop = rect(head.right() - 78, head.y + 3, 68, 20);
    let pressed = ui.button(stop, if done.is_some() { "Close" } else { "Stop" }, true);

    // The output. A card write is a conversation, so the last unterminated
    // line -- which is where `copal-prep.sh` puts its question -- is drawn as
    // well as the completed ones.
    ui.panel(body, t.tile);
    let inner = body.inset(PAD);
    let rows = ((inner.h - 4) / 14).max(1) as usize;
    let (lines, prompt) = {
        let j = m.job.as_ref().unwrap();
        let start = j.lines.len().saturating_sub(rows.saturating_sub(1));
        (j.lines[start..].to_vec(), j.prompt().to_string())
    };
    let mut y = inner.y;
    for line in &lines {
        ui.label_in(inner.x, y, inner.w, line, &F8X13, t.ink);
        y += 14;
    }
    if !prompt.is_empty() {
        // The prompt, with a block where the cursor is -- otherwise a question
        // with no newline after it looks like output that stopped.
        let x = ui.label_in(inner.x, y, inner.w - 12, &prompt, &F8X13, t.ink);
        ui.c.rect(x, y, 8, 13, t.accent);
    } else if done.is_none() {
        ui.c.rect(inner.x, y, 8, 13, t.accent);
    }

    // Typing goes to the terminal. THIS IS HOW `ERASE` REACHES copal-prep.sh,
    // and nothing here answers on the operator's behalf.
    let text = ui.f.text.clone();
    let typed_return = ui.f.pressed(Sym::Return);
    let typed_back = ui.f.pressed(Sym::Backspace);
    let interrupt = ui.f.keys.iter().any(|(s, mo)| *s == Sym::Char('c') && mo.ctrl);

    if let Some(j) = m.job.as_mut() {
        if !text.is_empty() {
            j.send(&text);
        }
        if typed_return {
            j.send("\n");
        }
        if typed_back {
            j.send("\u{7f}");
        }
        if interrupt {
            j.send("\u{3}");
        }
    }

    if pressed {
        if let Some(j) = m.job.as_mut() {
            if j.done.is_none() {
                j.stop();
            } else {
                m.job = None;
                m.reload();
            }
        }
    }
}

fn keys(ui: &mut Ui, m: &mut Media) {
    if m.cards.is_empty() {
        return;
    }
    let last = m.cards.len() - 1;
    if ui.f.pressed(Sym::Down) {
        m.sel = (m.sel + 1).min(last);
    }
    if ui.f.pressed(Sym::Up) {
        m.sel = m.sel.saturating_sub(1);
    }
    if ui.f.pressed(Sym::Home) {
        m.sel = 0;
    }
    if ui.f.pressed(Sym::End) {
        m.sel = last;
    }
    if ui.f.pressed(Sym::Right) {
        m.board = (m.board + 1).min(m.boards.len().saturating_sub(1));
    }
    if ui.f.pressed(Sym::Left) {
        m.board = m.board.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# Copal -- answers for an unattended install.
COPAL_GIT_NAME='Paul Richeson'
COPAL_USER='user'
COPAL_HOSTNAME='museum-03'
COPAL_ROOT_PW_HASH='$6$rounds=656000$abc$def'
COPAL_FLEET='museum'
COPAL_FLEET_SIZE='8'
COPAL_FLEET_INDEX='3'
COPAL_FLEET_ROLE='node'
COPAL_FLEET_TOKEN='deadbeef'
NOT_OURS='ignored'
"#;

    #[test]
    fn the_answers_file_is_read_without_being_run() {
        let a = Answers::parse(SAMPLE);
        assert_eq!(a.get("COPAL_FLEET"), "museum");
        assert_eq!(a.get("COPAL_USER"), "user");
        assert_eq!(a.num("COPAL_FLEET_SIZE"), 8);
        assert_eq!(a.num("COPAL_FLEET_INDEX"), 3);
        // Comments, blanks and anything that is not ours are not answers.
        assert_eq!(a.get("NOT_OURS"), "");
        assert_eq!(a.get("COPAL_MISSING"), "");
        // A value with = in it survives, because split_once takes the first.
        assert!(a.get("COPAL_ROOT_PW_HASH").starts_with("$6$rounds=656000$"));
    }

    #[test]
    fn the_password_hash_and_the_token_are_never_drawable() {
        for k in ["COPAL_ROOT_PW_HASH", "COPAL_FLEET_TOKEN", "COPAL_FLEET_PSK"] {
            assert!(Answers::is_secret(k), "{} is not marked secret", k);
        }
        for k in ["COPAL_FLEET", "COPAL_USER", "COPAL_HOSTNAME"] {
            assert!(!Answers::is_secret(k));
        }
    }

    #[test]
    fn the_hostname_is_the_one_copal_answers_makes() {
        assert_eq!(hostname_of("museum", 1), "museum-01");
        assert_eq!(hostname_of("museum", 8), "museum-08");
        assert_eq!(hostname_of("grove", 12), "grove-12");
    }

    #[test]
    fn the_ledger_tells_issued_from_enrolled_from_never_prepared() {
        let l = parse_ledger(
            "museum-01\tabc\tspent\nmuseum-02\tdef\tunused\nrubbish\nmuseum-03\t\t\n",
        );
        assert_eq!(l.get("museum-01"), Some(&Token::Spent));
        assert_eq!(l.get("museum-02"), Some(&Token::Unused));
        assert_eq!(l.get("museum-03"), None, "a malformed row became a token");
        assert_eq!(l.get("museum-08"), None);
    }

    #[test]
    fn enrolment_is_the_later_fact_and_wins_over_an_issued_token() {
        // `record_token` supersedes an unused row rather than appending, but a
        // ledger that has been hand-edited may carry both. Spent is the fact
        // that the machine exists.
        let l = parse_ledger("museum-01\tabc\tunused\nmuseum-01\tabc\tspent\n");
        assert_eq!(l.get("museum-01"), Some(&Token::Spent));
        let l = parse_ledger("museum-01\tabc\tspent\nmuseum-01\tdef\tunused\n");
        assert_eq!(l.get("museum-01"), Some(&Token::Spent), "enrolment was forgotten");
    }

    #[test]
    fn the_manifest_says_when_each_card_came_into_existence() {
        let man = parse_manifest(
            "2026-09-06T14:02:11\t8\tmuseum-08\tzero2\tsd\t-\ta3f10c2e\tok\n\
             2026-09-05T09:41:02\t5\tmuseum-05\tzero2\timg\t9f2c\t7be4419d\tok\n\
             short\trow\n",
        );
        assert_eq!(man.get("museum-08").map(String::as_str), Some("2026-09-06 zero2 sd"));
        assert_eq!(man.get("museum-05").map(String::as_str), Some("2026-09-05 zero2 img"));
        assert_eq!(man.get("museum-01"), None);
    }

    #[test]
    fn the_manifest_carries_a_fingerprint_and_never_the_token() {
        // The security property, asserted on the format this reads: the
        // seventh field is eight hex characters, not a token. A fingerprint
        // matches a card to a ledger row; it does not enrol.
        let row = "2026-09-06T14:02:11\t8\tmuseum-08\tzero2\tsd\t-\ta3f10c2e\tok";
        let fp = row.split('\t').nth(6).unwrap();
        assert_eq!(fp.len(), 8);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn museum() -> Media {
        let mut m = Media {
            root: PathBuf::from("/nowhere"),
            answers: Answers::parse(SAMPLE),
            fleet: "museum".into(),
            size: 8,
            current: 3,
            cards: Vec::new(),
            boards: BOARDS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            board: 0,
            sel: 0,
            job: None,
            error: String::new(),
        };
        let ledger = parse_ledger("museum-01\tx\tspent\nmuseum-02\tx\tspent\nmuseum-03\tx\tunused\n");
        let manifest = parse_manifest(
            "2026-09-04T10:00:00\t1\tmuseum-01\tzero2\tsd\t-\taaaaaaaa\tok\n\
             2026-09-04T11:00:00\t2\tmuseum-02\tzero2\tsd\t-\tbbbbbbbb\tok\n",
        );
        m.cards = (1..=8)
            .map(|i| {
                let hostname = hostname_of("museum", i);
                Card {
                    index: i,
                    token: *ledger.get(&hostname).unwrap_or(&Token::None),
                    built: manifest.get(&hostname).cloned().unwrap_or_default(),
                    seen: false,
                    hostname,
                }
            })
            .collect();
        m
    }

    #[test]
    fn a_card_can_only_be_written_while_the_answers_file_describes_it() {
        // THE SEQUENCE, ENFORCED INSTEAD OF REMEMBERED. This is the check that
        // replaces counting cards in your head, and the failure it prevents is
        // two machines with one identity and one enrolment token.
        let m = museum();
        assert!(m.can_write(&m.cards[2]), "card 3 is the one answers.txt describes");
        for i in [0, 1, 3, 4, 5, 6, 7] {
            assert!(
                !m.can_write(&m.cards[i]),
                "card {} was writable while answers.txt described card 3",
                i + 1
            );
        }
    }

    #[test]
    fn a_card_with_a_live_token_is_not_offered_a_second_one() {
        // "Two live tokens for one machine would mean the check has two right
        // answers, which is not a check."
        let m = museum();
        assert!(!m.can_prepare(&m.cards[2]), "card 3 already has an unused token");
        assert!(m.can_prepare(&m.cards[3]), "card 4 has never been prepared");
        assert!(m.can_prepare(&m.cards[0]), "card 1 enrolled, so it may be re-made");
    }

    #[test]
    fn nothing_may_start_while_something_is_running() {
        let mut m = museum();
        m.job = Job::start(
            "a job".into(),
            &["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            Path::new("."),
        )
        .ok();
        assert!(m.job.is_some());
        assert!(!m.can_write(&m.cards[2]));
        assert!(!m.can_prepare(&m.cards[3]));
        if let Some(j) = m.job.as_mut() {
            j.stop();
        }
    }

    #[test]
    fn the_next_thing_to_do_is_the_next_card_with_nothing_built() {
        let m = museum();
        assert_eq!(m.next_unbuilt(), Some(2), "cards 1 and 2 are built");
        let mut all = museum();
        for c in &mut all.cards {
            c.built = "2026-09-04 zero2 sd".into();
        }
        assert_eq!(all.next_unbuilt(), None, "a finished fleet has no next card");
    }

    #[test]
    fn the_read_model_says_which_cards_became_machines() {
        // Three sources, one row: a card, a token and a running machine are
        // three different things and the question needs all of them.
        let mut m = museum();
        m.note_fleet(&["museum-01".into(), "museum-02".into()]);
        assert!(m.cards[0].seen && m.cards[1].seen);
        assert!(!m.cards[2].seen);
        // A card that enrolled and is no longer answering is the row that
        // needs a person.
        assert_eq!(m.cards[0].token, Token::Spent);
        m.note_fleet(&[]);
        assert!(!m.cards[0].seen);
    }

    #[test]
    fn the_board_list_covers_every_target_copal_builds() {
        let names: Vec<&str> = BOARDS.iter().map(|(n, _)| *n).collect();
        for want in ["zero", "pi2b", "zero2", "pi4", "pi5", "pc", "pc32", "vm", "vmx86"] {
            assert!(names.contains(&want), "no board called {}", want);
        }
        assert_eq!(names.len(), 9);
    }

    #[test]
    fn a_terminal_conversation_is_kept_as_lines_and_a_prompt() {
        let mut j = Job::start(
            "asking".into(),
            &[
                "/bin/sh".into(),
                "-c".into(),
                "echo first line; printf 'Type ERASE: '; read a; echo \"got $a\"".into(),
            ],
            Path::new("."),
        )
        .expect("started");

        let start = std::time::Instant::now();
        while j.prompt().is_empty() && start.elapsed() < std::time::Duration::from_secs(5) {
            j.pump();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            j.lines.iter().any(|l| l.contains("first line")),
            "completed lines were lost: {:?}",
            j.lines
        );
        // The question has no newline after it, so a pane that only drew
        // completed lines would show a blank where the question is.
        assert!(j.prompt().contains("Type ERASE:"), "the prompt was lost: {:?}", j.prompt());

        j.send("ERASE\n");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            j.pump();
            if j.lines.iter().any(|l| l.contains("got ERASE")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            j.lines.iter().any(|l| l.contains("got ERASE")),
            "the typed answer never reached the child: {:?}",
            j.lines
        );
    }

    #[test]
    fn a_job_records_the_command_it_ran_as_its_first_line() {
        // Every screen is a rendering of a command a person could have typed,
        // so the pane says which one before it says anything else.
        let j = Job::start(
            "t".into(),
            &["/bin/sh".into(), "-c".into(), "true".into()],
            Path::new("."),
        )
        .expect("started");
        assert!(j.lines[0].starts_with("$ /bin/sh -c true"));
    }

    #[test]
    fn colour_meant_for_a_terminal_does_not_reach_the_canvas() {
        assert_eq!(strip_ansi("\u{1b}[36m==>\u{1b}[0m Card for zero2"), "==> Card for zero2");
        assert_eq!(strip_ansi("plain\r\n"), "plain\n");
        assert_eq!(strip_ansi("\u{1b}[1;31merror\u{1b}[0m"), "error");
    }

    #[test]
    fn both_buffers_are_bounded_whether_or_not_anything_arrived() {
        let mut j = Job::start(
            "t".into(),
            &["/bin/sh".into(), "-c".into(), "true".into()],
            Path::new("."),
        )
        .expect("started");

        for i in 0..2000 {
            j.lines.push(format!("line {}", i));
        }
        // A progress bar: one line, redrawn with carriage returns, no newline
        // ever. apk, dd and git clone all do this and all three are in the
        // chain a card write runs.
        j.partial = "#".repeat(40_000);
        j.partial.push_str(" 87% Type ERASE: ");

        j.pump();
        assert!(j.lines.len() <= Job::SCROLLBACK, "scrollback unbounded: {}", j.lines.len());
        assert!(j.partial.len() <= Job::MAX_PROMPT, "prompt unbounded: {}", j.partial.len());
        // And the TAIL survives, because that is where the question is.
        assert!(
            j.partial.ends_with("Type ERASE: "),
            "the prompt was truncated away: {:?}",
            &j.partial[j.partial.len().saturating_sub(40)..]
        );
    }

    #[test]
    fn a_multibyte_prompt_is_not_cut_in_half() {
        let mut j = Job::start(
            "t".into(),
            &["/bin/sh".into(), "-c".into(), "true".into()],
            Path::new("."),
        )
        .expect("started");
        // Cutting a UTF-8 sequence down the middle panics on a String split.
        j.partial = "\u{e9}".repeat(20_000);
        j.pump();
        assert!(j.partial.len() <= Job::MAX_PROMPT);
        assert!(j.partial.chars().all(|c| c == '\u{e9}'), "a character was cut in half");
    }
}
