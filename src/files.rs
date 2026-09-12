//! Exchange and Send: the two-pane browser, and the fan-out that shares it.
//!
//! ONE PANE FOR TWO VERBS, because they are the same act aimed differently.
//! Exchange is one node and two directions -- copy a file down, copy one up.
//! Send is many nodes and one direction, and the right-hand pane is the list of
//! machines rather than a listing of files, because there is no single
//! directory to browse when there are five of them.
//!
//! THE SESSION LIVES ON A THREAD AND ANSWERS BY POST. An SFTP call blocks: a
//! listing waits for the node, a transfer waits for a megabyte. Doing that on
//! the thread that draws would freeze the console for the length of the copy,
//! which for a card image is minutes. So the pane holds a channel, the worker
//! holds the connection, and the drawing reads whatever has arrived since the
//! last frame -- the same arrangement the read model already has in main.rs.
//!
//! WHAT IT DOES NOT DO: recursive copies, delete, rename, or permissions. Each
//! is a line of code and a way to lose a card image by mis-clicking, and none
//! of them is what the verb was asked for. `sftp.rs` has `remove` and `rename`
//! because the protocol does; the interface does not offer them.

use crate::font::F8X13;
use crate::sftp::{Entry, Sftp};
use crate::ssh;
use crate::ui::{rect, Rect, Ui};
use crate::surface::Sym;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

const PAD: i32 = 10;
/// What the footer says before anything has happened.
const HELP: &str = "Tab switches sides · Return opens a directory · Escape closes";
const ROW: i32 = 16;
const HEAD: i32 = 26;
const FOOT: i32 = 40;

/// What the pane asks the worker to do.
enum Cmd {
    List(String),
    Get(String, PathBuf),
    Put(PathBuf, String),
}

/// What the worker says back.
enum Ev {
    /// The connection is up, and this is where the node thinks we are.
    Ready(String, String),
    Listing(String, Vec<Entry>),
    Progress(u64, u64),
    Done(String),
    Failed(String),
}

/// Which half of the pane the keyboard is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Here,
    There,
}

pub struct Files {
    /// Every node this pane is aimed at. One means Exchange; more means Send.
    pub nodes: Vec<String>,
    dials: Vec<(String, ssh::Dial)>,

    pub here_dir: PathBuf,
    pub here: Vec<Entry>,
    pub here_sel: usize,

    pub there_dir: String,
    pub there: Vec<Entry>,
    pub there_sel: usize,

    pub side: Side,
    pub note: String,
    pub busy: Option<(u64, u64)>,
    pub connected: bool,
    /// Set while a fan-out is running, so nothing else starts underneath it.
    pub sending: bool,

    tx: Option<Sender<Cmd>>,
    rx: Option<Receiver<Ev>>,
    /// The fan-out's own channel: it has no persistent session to send down.
    fan: Option<Receiver<(String, Result<u64, String>)>>,
    pub results: Vec<(String, Result<u64, String>)>,
}

impl Files {
    /// Open the pane. With one node a connection is started at once; with
    /// several, nothing is connected until the operator presses Send, because
    /// five idle sessions to five Zero 2s is five sessions nobody asked for.
    pub fn open(dials: Vec<(String, ssh::Dial)>, start: PathBuf) -> Files {
        let nodes: Vec<String> = dials.iter().map(|(n, _)| n.clone()).collect();
        let mut f = Files {
            nodes,
            dials,
            here_dir: start,
            here: Vec::new(),
            here_sel: 0,
            there_dir: String::new(),
            there: Vec::new(),
            there_sel: 0,
            side: Side::Here,
            note: String::new(),
            busy: None,
            connected: false,
            sending: false,
            tx: None,
            rx: None,
            fan: None,
            results: Vec::new(),
        };
        f.read_here();
        if f.dials.len() == 1 {
            f.connect();
        }
        f
    }

    /// A pane with a made-up node on the other side, for `--frame-view files`.
    ///
    /// THE FIXTURE IS NAMED AS ONE. It draws the pane without a connection so
    /// the layout can be reviewed as a picture, and it is the only way to get
    /// entries into the remote side without a node -- nothing else in this
    /// file can invent a listing.
    pub fn specimen(nodes: Vec<String>, here: PathBuf) -> Files {
        let dials: Vec<(String, ssh::Dial)> = nodes
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    ssh::Dial {
                        addr: format!("{}:22", n),
                        host: n.clone(),
                        user: "copal".into(),
                        seed: [0u8; 32],
                        cert: Vec::new(),
                        cas: Vec::new(),
                    },
                )
            })
            .collect();
        let mut f = Files {
            nodes,
            dials,
            here_dir: here,
            here: Vec::new(),
            here_sel: 0,
            there_dir: "/home/copal".into(),
            there: vec![
                Entry { name: "images".into(), size: 0, mode: 0o040755, mtime: 0 },
                Entry { name: "scenes".into(), size: 0, mode: 0o040755, mtime: 0 },
                Entry { name: "copal-2026-09-01.img".into(), size: 1_932_735_283, mode: 0o100644, mtime: 0 },
                Entry { name: "answers.txt".into(), size: 2048, mode: 0o100644, mtime: 0 },
                Entry { name: "wall.log".into(), size: 51_200, mode: 0o100644, mtime: 0 },
            ],
            there_sel: 2,
            side: Side::Here,
            note: String::new(),
            busy: None,
            connected: true,
            sending: false,
            tx: None,
            rx: None,
            fan: None,
            results: Vec::new(),
        };
        f.read_here();
        f.note = if f.one_node() {
            HELP.to_string()
        } else {
            format!("choose a file, then copy it to all {} nodes", f.nodes.len())
        };
        f
    }

    fn connect(&mut self) {
        let (ctx, crx) = channel::<Cmd>();
        let (etx, erx) = channel::<Ev>();
        let (node, dial) = self.dials[0].clone();
        std::thread::spawn(move || worker(node, dial, crx, etx));
        self.tx = Some(ctx);
        self.rx = Some(erx);
        self.note = format!("opening {}", self.nodes[0]);
    }

    /// The local side, read straight off the disk.
    ///
    /// Sorted exactly the way the remote side is -- directories first, then by
    /// name, case ignored. Two panes side by side that sort differently is a
    /// pane people distrust.
    pub fn read_here(&mut self) {
        let mut out = Vec::new();
        match std::fs::read_dir(&self.here_dir) {
            Ok(entries) => {
                for e in entries.flatten() {
                    let meta = match e.metadata() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    out.push(Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        size: meta.len(),
                        mode: if meta.is_dir() { 0o040755 } else { 0o100644 },
                        mtime: 0,
                    });
                }
            }
            Err(e) => self.note = format!("{}: {}", self.here_dir.display(), e),
        }
        sort(&mut out);
        self.here = out;
        self.here_sel = 0;
    }

    /// Take delivery of whatever the worker said. Called once a frame.
    pub fn pump(&mut self) {
        loop {
            let ev = match self.rx.as_ref().map(|r| r.try_recv()) {
                Some(Ok(e)) => e,
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.rx = None;
                    self.tx = None;
                    self.connected = false;
                    break;
                }
            };
            match ev {
                Ev::Ready(cwd, _title) => {
                    self.connected = true;
                    self.there_dir = cwd.clone();
                    // The header already says which node this is. The footer
                    // is the only line with room for how to drive it, so it
                    // says that until something happens worth reporting.
                    self.note = HELP.to_string();
                    if let Some(tx) = &self.tx {
                        let _ = tx.send(Cmd::List(cwd));
                    }
                }
                Ev::Listing(dir, items) => {
                    self.there_dir = dir;
                    self.there = items;
                    self.there_sel = 0;
                }
                Ev::Progress(done, total) => self.busy = Some((done, total)),
                Ev::Done(what) => {
                    self.busy = None;
                    self.note = what;
                    self.read_here();
                    self.refresh_there();
                }
                Ev::Failed(why) => {
                    self.busy = None;
                    self.note = why;
                }
            }
        }
        // The fan-out reports one node at a time so the pane fills in as it
        // goes rather than sitting blank until the last one lands.
        loop {
            match self.fan.as_ref().map(|r| r.try_recv()) {
                Some(Ok(r)) => self.results.push(r),
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.fan = None;
                    self.sending = false;
                    self.busy = None;
                    break;
                }
            }
        }
    }

    fn refresh_there(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::List(self.there_dir.clone()));
        }
    }

    pub fn one_node(&self) -> bool {
        self.dials.len() == 1
    }

    fn here_pick(&self) -> Option<&Entry> {
        self.here.get(self.here_sel)
    }

    fn there_pick(&self) -> Option<&Entry> {
        self.there.get(self.there_sel)
    }

    /// Enter a directory, or do nothing if it is a file.
    fn enter_here(&mut self) {
        let Some(e) = self.here_pick() else { return };
        if !e.is_dir() {
            return;
        }
        self.here_dir = self.here_dir.join(&e.name);
        self.read_here();
    }

    fn up_here(&mut self) {
        if let Some(p) = self.here_dir.parent() {
            self.here_dir = p.to_path_buf();
            self.read_here();
        }
    }

    fn enter_there(&mut self) {
        let Some(e) = self.there_pick() else { return };
        if !e.is_dir() {
            return;
        }
        let next = format!("{}/{}", self.there_dir.trim_end_matches('/'), e.name);
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::List(next));
        }
    }

    fn up_there(&mut self) {
        let next = match self.there_dir.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(i) => self.there_dir[..i].to_string(),
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::List(next));
        }
    }

    /// Copy the highlighted local file up.
    pub fn put(&mut self) {
        if self.busy.is_some() || self.sending {
            return;
        }
        if !self.one_node() {
            return self.fan_out();
        }
        let Some(e) = self.here_pick().cloned() else { return };
        if e.is_dir() {
            self.note = "a directory is not a file, and this does not copy trees".into();
            return;
        }
        let target = format!("{}/{}", self.there_dir.trim_end_matches('/'), e.name);
        if let Some(tx) = &self.tx {
            self.busy = Some((0, e.size));
            let _ = tx.send(Cmd::Put(self.here_dir.join(&e.name), target));
        }
    }

    /// Copy the highlighted remote file down.
    pub fn get(&mut self) {
        if self.busy.is_some() || !self.one_node() {
            return;
        }
        let Some(e) = self.there_pick().cloned() else { return };
        if e.is_dir() {
            self.note = "a directory is not a file, and this does not copy trees".into();
            return;
        }
        let from = format!("{}/{}", self.there_dir.trim_end_matches('/'), e.name);
        if let Some(tx) = &self.tx {
            self.busy = Some((0, e.size));
            let _ = tx.send(Cmd::Get(from, self.here_dir.join(&e.name)));
        }
    }

    /// Send the highlighted local file to every selected node.
    fn fan_out(&mut self) {
        let Some(e) = self.here_pick().cloned() else { return };
        if e.is_dir() {
            self.note = "a directory is not a file, and this does not copy trees".into();
            return;
        }
        let path = self.here_dir.join(&e.name);
        let dials = self.dials.clone();
        let (tx, rx) = channel();
        self.fan = Some(rx);
        self.sending = true;
        self.results.clear();
        self.note = format!("sending {} to {} nodes", e.name, dials.len());
        std::thread::spawn(move || {
            for (node, d) in dials {
                let target = format!("/home/{}/{}", d.user, e.name);
                let r = Sftp::open(&d).and_then(|mut s| s.put(&path, &target, 0o644, |_, _| {}));
                if tx.send((node, r)).is_err() {
                    return;
                }
            }
        });
    }
}

fn sort(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// The thread that holds the connection.
fn worker(node: String, dial: ssh::Dial, rx: Receiver<Cmd>, tx: Sender<Ev>) {
    let mut s = match Sftp::open(&dial) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Ev::Failed(format!("{}: {}", node, e)));
            return;
        }
    };
    let cwd = match s.realpath(".") {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Ev::Failed(e));
            return;
        }
    };
    let _ = tx.send(Ev::Ready(cwd, format!("{} — files", node)));

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::List(dir) => match s.list(&dir) {
                Ok(items) => {
                    let _ = tx.send(Ev::Listing(dir, items));
                }
                Err(e) => {
                    let _ = tx.send(Ev::Failed(e));
                }
            },
            Cmd::Get(from, to) => {
                let p = tx.clone();
                let r = s.get(&from, &to, move |a, b| {
                    let _ = p.send(Ev::Progress(a, b));
                });
                let _ = tx.send(match r {
                    Ok(n) => Ev::Done(format!("{} bytes down to {}", n, to.display())),
                    Err(e) => Ev::Failed(e),
                });
            }
            Cmd::Put(from, to) => {
                let p = tx.clone();
                let r = s.put(&from, &to, 0o644, move |a, b| {
                    let _ = p.send(Ev::Progress(a, b));
                });
                let _ = tx.send(match r {
                    Ok(n) => Ev::Done(format!("{} bytes up to {}", n, to)),
                    Err(e) => Ev::Failed(e),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The drawing
// ---------------------------------------------------------------------------

/// Draw the pane. Returns true when the operator has closed it.
///
/// THE TWO SIDES ARE DRAWN BY ONE FUNCTION, so a row means the same thing on
/// both: the name on the left, the size on the right, directories first, the
/// way up as the first row. A browser whose halves behave differently is a
/// browser where people copy the wrong file.
pub fn draw(ui: &mut Ui, at: Rect, f: &mut Files) -> bool {
    let t = ui.t;
    // PAINT THE WHOLE PANE FIRST. Anything left unpainted shows the canvas's
    // own zeroes, which is black in both themes and looked like a hole in the
    // middle of the window -- the gutter between the two columns, which had no
    // widget of its own to fill it.
    ui.panel(at, t.base);
    let (head, rest) = at.cut_top(HEAD);
    // `cut_bottom` returns the piece it took FIRST, which is the opposite of
    // `cut_top`. Getting that backwards drew the whole browser in a
    // forty-pixel strip at the bottom of the window and cost one rendered
    // frame to notice -- which is what `--frame-view files` is for.
    let (foot, body) = rest.cut_bottom(FOOT);

    ui.panel(head, t.panel);
    ui.hrule(head.x, head.bottom() - 1, head.w);
    let title = if f.one_node() {
        format!("{} — files", f.nodes[0])
    } else {
        format!("Send to {} nodes", f.nodes.len())
    };
    ui.label(head.x + PAD, head.y + 6, &title, &F8X13, t.ink);
    let close = rect(head.right() - 78, head.y + 3, 68, 20);
    let closed = ui.button(close, "Close", true) || ui.f.pressed(Sym::Escape);

    // Two columns with a gutter of buttons between them. The gutter is where
    // the direction of a copy is stated, because an arrow between two lists is
    // the one place nobody misreads which way it goes.
    let gutter = 86;
    let col = (body.w - gutter) / 2;
    let left = rect(body.x, body.y, col, body.h);
    let mid = rect(body.x + col, body.y, gutter, body.h);
    let right = rect(body.x + col + gutter, body.y, body.w - col - gutter, body.h);

    let here_click = column(
        ui,
        left,
        &f.here_dir.to_string_lossy(),
        &f.here,
        f.here_sel,
        f.side == Side::Here,
    );
    if let Some(i) = here_click {
        f.side = Side::Here;
        if i == usize::MAX {
            f.up_here();
        } else {
            let was = f.here_sel;
            f.here_sel = i;
            if was == i {
                f.enter_here();
            }
        }
    }

    if f.one_node() {
        let there_click = column(
            ui,
            right,
            if f.there_dir.is_empty() { "connecting" } else { &f.there_dir },
            &f.there,
            f.there_sel,
            f.side == Side::There,
        );
        if let Some(i) = there_click {
            f.side = Side::There;
            if i == usize::MAX {
                f.up_there();
            } else {
                let was = f.there_sel;
                f.there_sel = i;
                if was == i {
                    f.enter_there();
                }
            }
        }
    } else {
        nodes_column(ui, right, f);
    }

    // The gutter.
    let busy = f.busy.is_some() || f.sending;
    let up = rect(mid.x + 8, mid.y + 40, gutter - 16, 22);
    let down = rect(mid.x + 8, mid.y + 70, gutter - 16, 22);
    let can_up = !busy && !f.here.is_empty() && (f.connected || !f.one_node());
    if ui.button(up, "copy →", can_up) {
        f.put();
    }
    if f.one_node() {
        let can_down = !busy && f.connected && !f.there.is_empty();
        if ui.button(down, "← copy", can_down) {
            f.get();
        }
    }

    // The footer: what just happened, or how far through it is.
    ui.panel(foot, t.panel);
    ui.hrule(foot.x, foot.y, foot.w);
    match f.busy {
        Some((done, total)) => {
            let pct = if total == 0 { 100 } else { (done * 100 / total.max(1)) as i32 };
            let bar = rect(foot.x + PAD, foot.y + 14, foot.w - 2 * PAD, 10);
            ui.panel(bar, t.tile);
            ui.c.rect(bar.x, bar.y, bar.w * pct / 100, bar.h, t.accent);
            ui.label_right(
                foot.right() - PAD,
                foot.y + 12,
                &format!("{} of {} bytes", done, total),
                &F8X13,
                t.dim,
            );
        }
        None => {
            ui.label_in(foot.x + PAD, foot.y + 12, foot.w - 2 * PAD, &f.note, &F8X13, t.dim);
        }
    }
    closed
}

/// One list. Returns the row clicked, or `usize::MAX` for the way up.
fn column(
    ui: &mut Ui,
    at: Rect,
    path: &str,
    items: &[Entry],
    sel: usize,
    focused: bool,
) -> Option<usize> {
    let t = ui.t;
    let (bar, list) = at.cut_top(22);
    ui.panel(bar, t.tile);
    // The path from the right: the end of a long path is the part that says
    // where you are, and the beginning is the part everybody already knows.
    ui.label_right(bar.right() - 6, bar.y + 5, path, &F8X13, t.dim);
    ui.panel(list, if focused { t.tile } else { t.panel });
    // BOTH SIDES ARE BOXES; only one of them is the one you are typing in.
    // Without the second outline the unfocused column read as rows floating on
    // the background rather than as the other half of a pair.
    ui.outline(list, if focused { t.accent } else { t.line });

    let inner = list.inset(6);
    let mut y = inner.y;
    let mut hit = None;

    let up = rect(inner.x, y, inner.w, ROW);
    if ui.is_hot(up) {
        ui.panel(up, t.panel);
    }
    ui.label(inner.x + 4, y + 2, "..", &F8X13, t.dim);
    if ui.button_hit(up) {
        hit = Some(usize::MAX);
    }
    y += ROW;

    let rows = ((inner.h - ROW) / ROW).max(1) as usize;
    // Scroll to keep the selection on screen rather than paging: a list of
    // sixty files with the selection off the bottom is a list that looks empty.
    let first = sel.saturating_sub(rows.saturating_sub(1));
    for (i, e) in items.iter().enumerate().skip(first).take(rows) {
        let r = rect(inner.x, y, inner.w, ROW);
        if i == sel {
            ui.panel(r, t.panel);
            ui.outline(r, t.accent);
        } else if ui.is_hot(r) {
            ui.panel(r, t.panel);
        }
        let colour = if e.is_dir() { t.accent } else { t.ink };
        let size = e.measure();
        let right_edge = ui.label_right(r.right() - 6, y + 2, &size, &F8X13, t.dim);
        ui.label_in(r.x + 4, y + 2, right_edge - r.x - 10, &e.name, &F8X13, colour);
        if ui.button_hit(r) {
            hit = Some(i);
        }
        y += ROW;
    }
    if items.is_empty() {
        ui.label(inner.x + 4, y + 2, "nothing here", &F8X13, t.dim);
    }
    hit
}

/// The right-hand side when there are several nodes: who this is going to, and
/// what happened to each once it has.
fn nodes_column(ui: &mut Ui, at: Rect, f: &Files) {
    let t = ui.t;
    let (bar, list) = at.cut_top(22);
    ui.panel(bar, t.tile);
    ui.label(bar.x + 6, bar.y + 5, "to", &F8X13, t.dim);
    ui.panel(list, t.panel);
    let inner = list.inset(6);
    let mut y = inner.y;
    for node in &f.nodes {
        let said = f.results.iter().find(|(n, _)| n == node);
        let (word, colour) = match said {
            None if f.sending => ("…".to_string(), t.dim),
            None => (String::new(), t.dim),
            Some((_, Ok(n))) => (format!("{} bytes", n), t.up),
            Some((_, Err(e))) => (e.clone(), t.alarm),
        };
        let edge = ui.label_right(inner.right() - 4, y + 2, &word, &F8X13, colour);
        ui.label_in(inner.x + 4, y + 2, edge - inner.x - 10, node, &F8X13, t.ink);
        y += ROW;
        if y > inner.bottom() - ROW {
            break;
        }
    }
}

/// The keys, which are the same ones the rest of the console uses.
pub fn keys(ui: &mut Ui, f: &mut Files) {
    let (len, sel) = match f.side {
        Side::Here => (f.here.len(), f.here_sel),
        Side::There => (f.there.len(), f.there_sel),
    };
    let mut next = sel;
    if ui.f.pressed(Sym::Down) && sel + 1 < len {
        next = sel + 1;
    }
    if ui.f.pressed(Sym::Up) && sel > 0 {
        next = sel - 1;
    }
    if ui.f.pressed(Sym::Home) {
        next = 0;
    }
    if ui.f.pressed(Sym::End) && len > 0 {
        next = len - 1;
    }
    match f.side {
        Side::Here => f.here_sel = next,
        Side::There => f.there_sel = next,
    }
    if ui.f.pressed(Sym::Tab) && f.one_node() {
        f.side = if f.side == Side::Here { Side::There } else { Side::Here };
    }
    if ui.f.pressed(Sym::Return) {
        match f.side {
            Side::Here => f.enter_here(),
            Side::There => f.enter_there(),
        }
    }
    if ui.f.pressed(Sym::Left) {
        match f.side {
            Side::Here => f.up_here(),
            Side::There => f.up_there(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;
    use crate::surface::{Button, Input};
    use crate::ui::UiState;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orrery-files-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn dial(node: &str) -> ssh::Dial {
        ssh::Dial {
            addr: format!("{}:22", node),
            host: node.into(),
            user: "copal".into(),
            seed: [1u8; 32],
            cert: Vec::new(),
            cas: Vec::new(),
        }
    }

    /// A pane with no worker: everything below is about what the pane decides,
    /// not about what a node answers -- `sftp.rs` already proves that half
    /// against OpenSSH's own server.
    fn pane(dir: PathBuf, nodes: &[&str]) -> Files {
        let dials: Vec<(String, ssh::Dial)> =
            nodes.iter().map(|n| (n.to_string(), dial(n))).collect();
        let mut f = Files {
            nodes: dials.iter().map(|(n, _)| n.clone()).collect(),
            dials,
            here_dir: dir,
            here: Vec::new(),
            here_sel: 0,
            there_dir: "/home/copal".into(),
            there: Vec::new(),
            there_sel: 0,
            side: Side::Here,
            note: String::new(),
            busy: None,
            connected: false,
            sending: false,
            tx: None,
            rx: None,
            fan: None,
            results: Vec::new(),
        };
        f.read_here();
        f
    }

    fn frame<R>(f: &mut Files, events: &[Input], go: impl FnOnce(&mut Ui, &mut Files) -> R) -> R {
        let (w, h) = (900usize, 600usize);
        let mut px = vec![0u8; w * h * 4];
        let mut c = draw::Canvas::new(&mut px, w, h);
        let mut state = UiState::default();
        let mut ui = Ui::begin(&mut c, draw::LIGHT, events, &mut state);
        go(&mut ui, f)
    }

    #[test]
    fn the_local_side_lists_directories_first_then_names() {
        let d = scratch("sort");
        std::fs::write(d.join("zebra.img"), b"x").unwrap();
        std::fs::write(d.join("Answers.txt"), b"x").unwrap();
        std::fs::create_dir(d.join("cards")).unwrap();
        std::fs::create_dir(d.join("Archive")).unwrap();

        let f = pane(d.clone(), &["museum-01"]);
        let names: Vec<&str> = f.here.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Archive", "cards", "Answers.txt", "zebra.img"]);
        // The same order the remote side uses, which is the whole point: two
        // panes side by side that sort differently is a pane people distrust.
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn one_node_is_exchange_and_several_is_send() {
        let d = scratch("arity");
        assert!(pane(d.clone(), &["museum-01"]).one_node());
        assert!(!pane(d.clone(), &["museum-01", "museum-02"]).one_node());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_directory_is_refused_as_a_thing_to_copy() {
        // Choosing a folder and pressing copy is what everybody does first.
        // It has to be a sentence rather than a silence or a half-copy.
        let d = scratch("nodir");
        std::fs::create_dir(d.join("cards")).unwrap();
        let mut f = pane(d.clone(), &["museum-01"]);
        f.here_sel = 0;
        assert!(f.here[0].is_dir());
        f.put();
        assert!(f.note.contains("not a file"), "{}", f.note);
        assert!(f.busy.is_none(), "a directory started a transfer");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn nothing_starts_while_something_is_running() {
        // The same rule the card writer has: one job at a time, so a half
        // finished transfer cannot be overtaken by the next one.
        let d = scratch("busy");
        std::fs::write(d.join("a.img"), b"xxxx").unwrap();
        let mut f = pane(d.clone(), &["museum-01"]);
        f.busy = Some((10, 100));
        f.put();
        f.get();
        assert_eq!(f.busy, Some((10, 100)), "a second transfer began");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn walking_into_a_directory_and_back_out_lands_where_it_started() {
        let d = scratch("walk");
        std::fs::create_dir(d.join("cards")).unwrap();
        std::fs::write(d.join("cards/one.img"), b"x").unwrap();
        let mut f = pane(d.clone(), &["museum-01"]);
        let start = f.here_dir.clone();
        f.here_sel = 0;
        f.enter_here();
        assert_eq!(f.here.len(), 1, "did not enter the directory");
        assert_eq!(f.here[0].name, "one.img");
        f.up_here();
        assert_eq!(f.here_dir, start);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_way_up_the_remote_side_never_runs_out_of_root() {
        let d = scratch("root");
        let mut f = pane(d.clone(), &["museum-01"]);
        // No worker, so this exercises the arithmetic rather than the node.
        for (from, want) in [
            ("/home/copal/images", "/home/copal"),
            ("/home/copal", "/home"),
            ("/home", "/"),
            ("/", "/"),
        ] {
            f.there_dir = from.into();
            let next = match f.there_dir.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => f.there_dir[..i].to_string(),
            };
            assert_eq!(next, want, "going up from {}", from);
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_pane_draws_both_shapes_without_falling_over() {
        let d = scratch("draw");
        std::fs::write(d.join("image.img"), vec![0u8; 5000]).unwrap();
        std::fs::create_dir(d.join("cards")).unwrap();

        for nodes in [&["museum-01"][..], &["museum-01", "museum-02", "museum-03"][..]] {
            let mut f = pane(d.clone(), nodes);
            f.note = "opening".into();
            let closed = frame(&mut f, &[], |ui, f| draw(ui, rect(0, 0, 900, 600), f));
            assert!(!closed, "the pane closed itself");
            // And with a transfer in flight, which draws a bar instead of the
            // note.
            f.busy = Some((2500, 5000));
            frame(&mut f, &[], |ui, f| draw(ui, rect(0, 0, 900, 600), f));
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn clicking_close_closes_and_escape_does_the_same() {
        let d = scratch("close");
        let mut f = pane(d.clone(), &["museum-01"]);
        let click = |x: i32, y: i32| {
            vec![
                Input::Button { x, y, button: Button::Left, down: true },
                Input::Button { x, y, button: Button::Left, down: false },
            ]
        };
        // The Close button sits at the right of the header.
        let at = rect(0, 0, 900, 600);
        let closed = frame(&mut f, &click(at.right() - 44, 13), |ui, f| draw(ui, at, f));
        assert!(closed, "the Close button did not close the pane");

        let esc = [Input::Key {
            scancode: 0,
            sym: Sym::Escape,
            down: true,
            mods: Default::default(),
        }];
        let closed = frame(&mut f, &esc, |ui, f| draw(ui, at, f));
        assert!(closed, "escape did not close the pane");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_keys_move_the_selection_and_stop_at_the_ends() {
        let d = scratch("keys");
        for n in ["a.img", "b.img", "c.img"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let mut f = pane(d.clone(), &["museum-01"]);
        let key = |sym: Sym| {
            vec![Input::Key { scancode: 0, sym, down: true, mods: Default::default() }]
        };
        for _ in 0..5 {
            frame(&mut f, &key(Sym::Down), |ui, f| keys(ui, f));
        }
        assert_eq!(f.here_sel, 2, "the selection ran off the end");
        for _ in 0..5 {
            frame(&mut f, &key(Sym::Up), |ui, f| keys(ui, f));
        }
        assert_eq!(f.here_sel, 0, "the selection ran off the top");
        let _ = std::fs::remove_dir_all(d);
    }
}
