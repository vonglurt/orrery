//! The Fleet screen: who may command these machines, what each of them is, and
//! what the commands can do today.
//!
//! Paul, 30 October: "there should be whole screen dedicated to orchestrating and
//! describes its capabilities and CA." The Lab is a room to look into, one machine
//! or a handful picked by eye. THIS SCREEN STARTS FROM THE QUESTION INSTEAD: which
//! machines are Ubuntu, idle, on that commit? It asks them, lights the answer, and
//! a verb goes to what lit.
//!
//! THE AUTHORITY IS AT THE TOP BECAUSE IT DECIDES EVERYTHING BELOW. Every button
//! here is a certificate presented to a forced command, so when one expires or a
//! machine has no listener, this screen says that rather than letting a button fail
//! by surprise.
//!
//! IT DRAWS; IT DOES NOT DECIDE. `tools/orchestrate.py` writes the document and reads
//! the selection words, so the screen and `fleet-select.py` cannot come to speak
//! different languages. Installing needs root, so it is staged as Apply is: copied,
//! typed into the machine's Sessions tile, the password typed there by the person.
//! Renewing runs `copal fleet login`; orrery never creates a credential.
//!
//! A BUTTON THAT CANNOT WORK SAYS WHY, in the capability table, with a count of the
//! machines where it can. Control and Snapshot read 0 today, and that is the point.

use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::ui::{rect, Field, Ui};

const PAD: i32 = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub addr: String,
    pub os: String,
    pub version: String,
    pub cpus: String,
    pub load: String,
    pub idle: bool,
    pub treadcore: String,
    pub gpu: String,
    pub listener: String,
    pub desktop: String,
    pub cert_days: i64,
    pub cert_ours: bool,
    pub last_message: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capability {
    pub name: String,
    pub does: String,
    pub ready: i64,
    pub total: i64,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub why: String,
    pub fleet: String,
    pub surveyed: String,
    pub ca_fingerprint: String,
    pub ca_ok: bool,
    pub principals: Vec<String>,
    /// Minutes left on the operator's certificate; negative when there is none.
    pub left_min: i64,
    pub machines: Vec<Machine>,
    pub buttons: Vec<Capability>,
}

fn s(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn n(v: &json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0) as i64
}

fn yes(v: &json::Value, k: &str) -> bool {
    matches!(v.get(k), Some(json::Value::Bool(true)))
}

impl Model {
    pub fn parse(text: &str) -> Model {
        let v = match json::parse(text.trim()) {
            Ok(v) if v.get("machines").is_some() => v,
            _ => return Model { why: text.trim().lines().last().unwrap_or("asking the fleet...").to_string(), left_min: -1, ..Model::default() },
        };
        let mut m = Model { fleet: s(&v, "fleet"), surveyed: s(&v, "surveyed"), left_min: -1, ..Model::default() };
        if let Some(ca) = v.get("ca") {
            m.ca_fingerprint = s(ca, "fingerprint");
            m.ca_ok = yes(ca, "ok");
        }
        if let Some(op) = v.get("operator") {
            m.left_min = n(op, "left_min");
            if let Some(a) = op.get("principals").and_then(|x| x.as_array()) {
                m.principals = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            }
        }
        for x in v.get("machines").and_then(|x| x.as_array()).unwrap_or(&[]) {
            m.machines.push(Machine {
                name: s(x, "name"),
                addr: s(x, "addr"),
                os: s(x, "os"),
                version: s(x, "version"),
                cpus: s(x, "cpus"),
                load: s(x, "load"),
                idle: yes(x, "idle"),
                treadcore: s(x, "treadcore"),
                gpu: s(x, "gpu"),
                listener: s(x, "listener"),
                desktop: s(x, "desktop"),
                cert_days: x.get("cert_days").and_then(|d| d.as_num()).unwrap_or(-1.0) as i64,
                cert_ours: yes(x, "cert_ours"),
                last_message: s(x, "last_message"),
                error: s(x, "error"),
            });
        }
        for x in v.get("buttons").and_then(|x| x.as_array()).unwrap_or(&[]) {
            m.buttons.push(Capability { name: s(x, "name"), does: s(x, "does"), ready: n(x, "ready"), total: n(x, "total"), why: s(x, "why") });
        }
        m
    }
}

/// What the person pressed, for main to run on a thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Survey,
    Login,
    Select(String),
    /// The listener recipe staged on this machine, typed in its tile.
    Install(String),
    Send { nodes: Vec<String>, verb: String },
    /// Text on stdin: `clip`, or `save NAME`. None: this Mac's clipboard.
    Text { nodes: Vec<String>, verb: String, text: Option<String> },
    /// A picture of each machine's screen, saved on this Mac.
    Shot(Vec<String>),
    /// A line in Fleet Chat.
    Say(String),
}

/// Fleet Chat, as `orchestrate.py chat` gave it: the room, and who can be reached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Room {
    pub count: i64,
    pub lines: Vec<String>,
    pub machines: Vec<(String, bool)>,
}

pub fn parse_room(text: &str) -> Option<Room> {
    let v = json::parse(text.trim()).ok()?;
    v.get("machines")?;
    Some(Room {
        count: n(&v, "count"),
        lines: v.get("lines").and_then(|x| x.as_array()).unwrap_or(&[]).iter().filter_map(|l| l.as_str().map(str::to_string)).collect(),
        machines: v.get("machines").and_then(|x| x.as_array()).unwrap_or(&[]).iter().map(|m| (s(m, "name"), yes(m, "ok"))).collect(),
    })
}

/// A long line, in the rows a width of characters holds.
pub fn wrap(line: &str, width: usize) -> Vec<String> {
    let cs: Vec<char> = line.chars().collect();
    if cs.is_empty() {
        return vec![String::new()];
    }
    cs.chunks(width.max(1)).map(|c| c.iter().collect()).collect()
}

/// The console's own clipboard, one line of it: the field holds one line.
fn console_clipboard() -> String {
    for (cmd, args) in [("pbpaste", &[][..]), ("wl-paste", &["-n"][..])] {
        if let Ok(o) = std::process::Command::new(cmd).args(args).output() {
            if o.status.success() {
                return String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
            }
        }
    }
    String::new()
}

/// The node's rule for a file's name, kept here so a bad one fails at the prompt.
pub fn good_file_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && !n.starts_with('.')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Default)]
pub struct State {
    pub operator: bool,
    query: Field,
    arg: Field,
    /// The second field has the keyboard.
    typing_arg: bool,
    pub chosen: Vec<String>,
    /// orchestrate.py's last answer, as it came.
    pub reply: String,
    seen: String,
    /// One line a machine, from the last verb; or the last thing said.
    pub lines: Vec<(String, bool, String)>,
    pub said: String,
    pub unasked: Vec<String>,
    /// A verb that stops a machine, waiting for its OK.
    confirm: Option<Ask>,
    /// Save... is asking what to call the file.
    naming: Option<Field>,
    pub ask: Option<Ask>,
    /// Fleet Chat: the line being typed, and where the transcript is scrolled.
    chat_line: Field,
    chat_off: usize,
    chat_at_end: bool,
    chat_started: bool,
}

impl State {
    pub fn new(operator: bool) -> State {
        State { operator, ..State::default() }
    }

    /// Take the tool's answer once: a selection lights machines, a verb's results
    /// become lines, anything else is said.
    pub fn take_reply(&mut self) {
        if self.reply == self.seen {
            return;
        }
        self.seen = self.reply.clone();
        // Cleared by the press that asked: nothing yet.
        if self.reply.is_empty() {
            return;
        }
        let Ok(v) = json::parse(self.reply.trim()) else {
            self.said = self.reply.trim().lines().last().unwrap_or("").to_string();
            return;
        };
        if let Some(a) = v.get("chosen").and_then(|x| x.as_array()) {
            self.chosen = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            self.unasked = v.get("unasked").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
            self.said = if self.unasked.is_empty() {
                format!("{} matched", self.chosen.len())
            } else {
                format!("{} matched; {} could not be asked: {}", self.chosen.len(), self.unasked.len(), self.unasked.join(", "))
            };
            self.lines.clear();
        } else if let Some(a) = v.get("results").and_then(|x| x.as_array()) {
            self.lines = a.iter().map(|r| (s(r, "node"), yes(r, "ok"), s(r, "out"))).collect();
            let good = self.lines.iter().filter(|l| l.1).count();
            self.said = format!("{}: {} of {} answered", s(&v, "verb").split(' ').next().unwrap_or(""), good, self.lines.len());
        } else {
            self.said = s(&v, "said");
        }
    }
}

/// The certificate's time left, the way a person reads a clock.
fn left(min: i64) -> String {
    if min < 0 {
        "expired or none".into()
    } else if min < 60 {
        format!("{} min left", min)
    } else {
        format!("{} h {} min left", min / 60, min % 60)
    }
}

/// A selection's chips: the common questions, as the words they are.
const CHIPS: [(&str, &str); 6] =
    [("All", ""), ("Ubuntu", "os=ubuntu"), ("Alpine", "os=alpine"), ("Idle", "idle"), ("GPU", "gpu"), ("Ready", "")];

pub fn draw(ui: &mut Ui, m: &Model, st: &mut State) -> Option<nav::View> {
    let t = ui.t;
    st.take_reply();
    let free = st.confirm.is_none() && st.naming.is_none();
    let (rest, go, _) = nav::header(ui, nav::View::Fleet, free, "the authority, the machines, the verbs", None);
    let body = rest.inset(PAD);
    let lh = F8X13.height as i32 + 6;
    if !m.why.is_empty() {
        ui.label_in(body.x, body.y + 8, body.w, &m.why, &F8X13, t.dim);
        return go;
    }

    // ---- the authority -----------------------------------------------------------
    let band = rect(body.x, body.y, body.w, lh * 4 + 12);
    ui.panel(band, t.tile);
    ui.outline(band, if m.left_min > 30 { t.line } else { t.warn });
    let (ax, mut y) = (band.x + 10, band.y + 6);
    let aw = band.w * 3 / 5 - 14;
    ui.label_in(ax, y, aw, &format!("Certificate authority: {}", m.fleet), &F8X13, t.ink);
    y += lh;
    ui.label_in(ax, y, aw, if m.ca_ok { &m.ca_fingerprint } else { "no CA on this Mac" }, &F8X13, t.dim);
    y += lh;
    let who = if m.principals.is_empty() { "no operator certificate".to_string() } else { m.principals.join(", ") };
    ui.label_in(ax, y, aw - 96, &format!("You: {}, {}", who, left(m.left_min)), &F8X13, if m.left_min > 30 { t.ink } else { t.warn });
    if st.operator && free && ui.button(rect(ax + aw - 90, y - 4, 90, 22), "Renew", m.ca_ok) {
        st.ask = Some(Ask::Login);
    }
    y += lh;
    let up = m.machines.iter().filter(|x| x.listener == "yes").count();
    let signed = m.machines.iter().filter(|x| x.cert_ours).count();
    ui.label_in(ax, y, aw - 96, &format!("{} of {} machines listening, {} show this CA's", up, m.machines.len(), signed), &F8X13, t.ink);
    if st.operator && free && ui.button(rect(ax + aw - 90, y - 4, 90, 22), "Survey", true) {
        st.ask = Some(Ask::Survey);
    }
    // The argument, beside the facts it explains.
    let (rx, rw) = (band.x + band.w * 3 / 5 + 4, band.w * 2 / 5 - 14);
    let mut ry = band.y + 6;
    for line in [
        "No passwords: the CA signs one short",
        "certificate for you and one per machine, so",
        "each proves itself. fleet-operator reaches a",
        "fixed list of verbs; fleet-human, a shell.",
    ] {
        ui.label_in(rx, ry, rw, line, &F8X13, t.dim);
        ry += lh;
    }

    // ---- the question ------------------------------------------------------------
    let mut y = band.bottom() + 8;
    ui.label(body.x, y + 4, "Select", &F8X13, t.ink);
    let chip_w = 72;
    let chips_w = chip_w * CHIPS.len() as i32 + 96;
    let qr = rect(body.x + 54, y, (body.w - 54 - chips_w - 12).max(120), 22);
    if free && ui.button_hit(qr) {
        st.typing_arg = false;
    }
    ui.field(qr, &st.query, "os=ubuntu idle   treadcore>=2026-10-29   has:tmux   or ...");
    if !st.typing_arg {
        ui.outline(qr, t.accent);
    }
    let entered = free && !st.typing_arg && st.query.feed(&ui.f);
    let mut x = qr.right() + 8;
    if (ui.button(rect(x, y, 88, 22), "Ask", !st.query.text.is_empty() && free) || entered) && !st.query.text.is_empty() {
        st.ask = Some(Ask::Select(st.query.text.clone()));
    }
    x += 96;
    for (label, words) in CHIPS {
        if ui.button(rect(x, y, chip_w - 4, 22), label, free) {
            match label {
                "All" => st.chosen = m.machines.iter().map(|x| x.name.clone()).collect(),
                "Ready" => st.chosen = m.machines.iter().filter(|x| x.listener == "yes").map(|x| x.name.clone()).collect(),
                _ => {
                    st.query = Field::new(words);
                    st.ask = Some(Ask::Select(words.to_string()));
                }
            }
        }
        x += chip_w;
    }
    y += 30;

    // ---- the foot: verbs and what they said ------------------------------------------
    let foot_h = lh * 2 + 8 + lh * 4;
    let (foot, mid) = rect(body.x, y, body.w, body.bottom() - y).cut_bottom(foot_h);

    // ---- the machines, and the capabilities beside them -----------------------------------
    // A narrow window keeps every column and gives up the commit's date: columns
    // that ran on under the capability table read as one garbled line (960 wide).
    let wide = mid.w >= 1000;
    let cap_w = if wide { 340 } else { 280 };
    let tw = mid.w - cap_w - 12;
    let mut y = mid.y;
    let cols = if wide { [24, 132, 244, 322, 474, 570] } else { [24, 128, 232, 296, 362, 430] };
    let head = ["machine", "system", "load", if wide { "treadcore" } else { "commit" }, "certificate"];
    for (i, h) in head.iter().enumerate() {
        ui.label(mid.x + cols[i], y, h, &F8X13, t.dim);
    }
    y += lh;
    ui.hrule(mid.x, y - 3, tw);
    for x in &m.machines {
        if y + lh > mid.bottom() {
            break;
        }
        let r = rect(mid.x, y - 2, tw, lh);
        let on = st.chosen.contains(&x.name);
        if on {
            ui.panel(r, t.tile);
            ui.outline(r, t.up);
        }
        let lamp = if x.listener == "yes" { t.up } else if x.listener == "no" || x.listener == "old" { t.warn } else { t.alarm };
        ui.c.disc(r.x + 11, r.y + r.h / 2, 4, lamp);
        let ty = y + 1;
        let lim = |i: usize| cols[i + 1] - cols[i] - 6;
        ui.label_in(r.x + cols[0], ty, lim(0), &x.name, &F8X13, t.ink);
        ui.label_in(r.x + cols[1], ty, lim(1), &format!("{} {}", x.os, x.version), &F8X13, t.dim);
        let load = if x.load.is_empty() { "-".to_string() } else { format!("{}/{}", x.load, x.cpus) };
        ui.label_in(r.x + cols[2], ty, lim(2), &load, &F8X13, if x.idle { t.up } else { t.dim });
        ui.label_in(r.x + cols[3], ty, lim(3), &x.treadcore, &F8X13, t.dim);
        let cert = if x.cert_ours { format!("{} days", x.cert_days) } else if x.cert_days >= 0 { "another CA".to_string() } else { "none".to_string() };
        ui.label_in(r.x + cols[4], ty, (r.w - cols[4] - 72).max(30), &cert, &F8X13, if x.cert_ours && x.cert_days > 14 { t.dim } else { t.warn });
        // THE ROW STOPS WHERE THE BUTTON STARTS. Two widgets under one press is the
        // later one's press, so a row round its own button takes every click that
        // does not begin and end inside a single frame (30 October).
        let mut row = r;
        if st.operator && free && x.listener != "yes" {
            let b = rect(r.right() - 70, r.y + 1, 68, r.h - 2);
            if ui.button(b, if x.listener == "old" { "Update" } else { "Install" }, true) {
                st.ask = Some(Ask::Install(x.name.clone()));
            }
            row.w -= 72;
        }
        if free && ui.button_hit(row) {
            if on {
                st.chosen.retain(|n| n != &x.name);
            } else {
                st.chosen.push(x.name.clone());
            }
        }
        y += lh;
    }

    let cx = mid.x + tw + 12;
    let mut y = mid.y;
    ui.label(cx, y, "What the buttons can do", &F8X13, t.ink);
    ui.label_right(cx + cap_w, y, "machines", &F8X13, t.dim);
    y += lh;
    ui.hrule(cx, y - 3, cap_w);
    let rows = (m.buttons.len() as i32).max(1);
    let two = (mid.bottom() - y) / rows >= lh * 2 - 2;
    for b in &m.buttons {
        if y + lh > mid.bottom() {
            break;
        }
        let c = if b.ready == b.total && b.total > 0 { t.up } else if b.ready == 0 { t.warn } else { t.ink };
        ui.label_in(cx, y, 76, &b.name, &F8X13, t.ink);
        ui.label_right(cx + cap_w, y, &format!("{} of {}", b.ready, b.total), &F8X13, c);
        let text = if b.why.is_empty() { &b.does } else { &b.why };
        if two {
            ui.label_in(cx + 8, y + lh - 3, cap_w - 8, text, &F8X13, t.dim);
            y += lh * 2 - 2;
        } else {
            ui.label_in(cx + 80, y, cap_w - 80 - 66, text, &F8X13, t.dim);
            y += lh;
        }
    }

    // ---- the verbs, to what is lit -------------------------------------------------------
    let mut y = foot.y + 4;
    let some = !st.chosen.is_empty() && st.operator && free;
    ui.label(foot.x, y + 4, &format!("To {}", st.chosen.len()), &F8X13, t.ink);
    let fx = foot.x + 54;
    let bw = 72;
    let bar = bw * 8 + 16;
    let ar = rect(fx, y, (foot.w - 54 - bar - 8).max(120), 22);
    if free && ui.button_hit(ar) {
        st.typing_arg = true;
    }
    ui.field(ar, &st.arg, "a message, scene or verb; Clip and Save take this, or the Mac's clipboard");
    if st.typing_arg {
        ui.outline(ar, t.accent);
        if free {
            st.arg.feed(&ui.f);
        }
    }
    let mut x = ar.right() + 8;
    let arg = st.arg.text.trim().to_string();
    let send = |verb: String, st: &mut State| st.ask = Some(Ask::Send { nodes: st.chosen.clone(), verb });
    if ui.button(rect(x, y, bw - 4, 22), "Message", some && !arg.is_empty()) {
        send(format!("message {}", arg), st);
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Run", some && !arg.is_empty()) {
        send(arg.clone(), st);
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Scene", some && !arg.is_empty()) {
        send(format!("scene apply {}", arg), st);
    }
    x += bw;
    let typed = if arg.is_empty() { None } else { Some(arg.clone()) };
    if ui.button(rect(x, y, bw - 4, 22), "Clip", some) {
        st.ask = Some(Ask::Text { nodes: st.chosen.clone(), verb: "clip".into(), text: typed.clone() });
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Save...", some) {
        st.naming = Some(Field::default());
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Shot", some) {
        st.ask = Some(Ask::Shot(st.chosen.clone()));
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Reboot", some) {
        st.confirm = Some(Ask::Send { nodes: st.chosen.clone(), verb: "power reboot".into() });
    }
    x += bw;
    if ui.button(rect(x, y, bw - 4, 22), "Off", some) {
        st.confirm = Some(Ask::Send { nodes: st.chosen.clone(), verb: "power off".into() });
    }
    y += lh + 8;
    ui.label_in(foot.x, y, foot.w, &st.said, &F8X13, t.ink);
    y += lh;
    for (node, ok, out) in &st.lines {
        if y + lh > foot.bottom() {
            break;
        }
        ui.c.disc(foot.x + 6, y + 6, 4, if *ok { t.up } else { t.alarm });
        ui.label_in(foot.x + 18, y, 110, node, &F8X13, t.ink);
        ui.label_in(foot.x + 130, y, foot.w - 130, out, &F8X13, t.dim);
        y += lh;
    }

    // ---- the file's name ------------------------------------------------------------------
    if let Some(mut name) = st.naming.take() {
        let pane = rect(body.x + body.w / 6, body.y + body.h / 4, body.w * 2 / 3, 170);
        ui.panel(pane, t.tile);
        ui.outline(pane, t.accent);
        let (ix, iw) = (pane.x + 16, pane.w - 32);
        ui.label_in(ix, pane.y + 14, iw, "Save as a file", &F10X20, t.ink);
        let from = if arg.is_empty() { "this Mac's clipboard".to_string() } else { format!("the {} characters typed", arg.chars().count()) };
        ui.label_in(ix, pane.y + 46, iw, &format!("{}, to ~/fleet-inbox on {}", from, st.chosen.join(", ")), &F8X13, t.dim);
        let fr = rect(ix, pane.y + 46 + lh, iw, 24);
        ui.field(fr, &name, "notes.txt");
        ui.outline(fr, t.accent);
        let entered = name.feed(&ui.f);
        let ok = good_file_name(name.text.trim());
        let by = pane.bottom() - 36;
        let cancel = ui.button(rect(ix + 120, by, 90, 24), "Cancel", true) || ui.f.pressed(crate::surface::Sym::Escape);
        if (ui.button(rect(ix, by, 110, 24), "Save", ok) || entered) && ok {
            st.ask = Some(Ask::Text { nodes: st.chosen.clone(), verb: format!("save {}", name.text.trim()), text: typed });
        } else if !cancel {
            st.naming = Some(name);
        }
    }

    // ---- the confirmation ----------------------------------------------------------------
    if let Some(Ask::Send { nodes, verb }) = st.confirm.clone() {
        let pane = rect(body.x + body.w / 6, body.y + body.h / 4, body.w * 2 / 3, 150);
        ui.panel(pane, t.tile);
        ui.outline(pane, t.alarm);
        let (ix, iw) = (pane.x + 16, pane.w - 32);
        ui.label_in(ix, pane.y + 14, iw, if verb.ends_with("off") { "Power off" } else { "Reboot" }, &F10X20, t.ink);
        ui.label_in(ix, pane.y + 46, iw, &nodes.join(", "), &F8X13, t.ink);
        ui.label_in(ix, pane.y + 46 + lh, iw, "Whatever is running on them stops; the queue runs its jobs again.", &F8X13, t.dim);
        let by = pane.bottom() - 36;
        if ui.button(rect(ix, by, 110, 24), "Do it", true) {
            st.ask = st.confirm.take();
        } else if ui.button(rect(ix + 120, by, 90, 24), "Cancel", true) || ui.f.pressed(crate::surface::Sym::Escape) {
            st.confirm = None;
        }
    }
    go
}

/// Fleet Chat. ONE ROOM, not a list of conversations: every machine and this console
/// read the same lines, and this console keeps them. NOTHING BUT THE CHAT: what was
/// said with a scroll bar, a line to type in, a way to paste, and who can be reached.
pub fn draw_chat(ui: &mut Ui, room: Option<&Room>, st: &mut State) -> Option<nav::View> {
    let t = ui.t;
    let (rest, go, _) = nav::header(ui, nav::View::Chat, true, "one room for the whole fleet", None);
    let body = rest.inset(PAD);
    let lh = F8X13.height as i32 + 6;
    let Some(room) = room else {
        ui.label_in(body.x, body.y + 8, body.w, "asking the machines...", &F8X13, t.dim);
        return go;
    };
    if !st.chat_started {
        st.chat_started = true;
        st.chat_at_end = true;
    }
    // Who is in reach, a lamp each, along the top.
    let mut x = body.x;
    for (name, ok) in &room.machines {
        let w = F8X13.measure(name) as i32 + 26;
        if x + w > body.right() {
            break;
        }
        ui.c.disc(x + 6, body.y + 8, 4, if *ok { t.up } else { t.warn });
        ui.label(x + 14, body.y + 2, name, &F8X13, t.dim);
        x += w;
    }
    let bh = 28;
    let top = body.y + lh + 2;
    let box_ = rect(body.x, top, body.w, body.bottom() - top - bh - 8);
    ui.panel(box_, t.tile);
    let cols = ((body.w - 32) / F8X13.measure("m").max(1) as i32).max(10) as usize;
    let rows: Vec<String> = if room.lines.is_empty() {
        vec!["Nothing yet. Say something here, or run fleet-chat on any machine.".to_string()]
    } else {
        room.lines.iter().flat_map(|l| wrap(l, cols)).collect()
    };
    let inner = box_.inset(6);
    if st.chat_at_end {
        st.chat_off = rows.len();
    }
    let mut off = st.chat_off;
    let (area, first, fit) = ui.scroll_list(inner, rows.len(), lh, &mut off);
    st.chat_off = off;
    // At the end it follows what arrives; scrolled up, it stays where you are reading.
    st.chat_at_end = first + fit >= rows.len();
    let mut ty = area.y;
    for l in rows.iter().skip(first).take(fit) {
        ui.label_in(area.x, ty, area.w, l, &F8X13, t.ink);
        ty += lh;
    }
    let by = box_.bottom() + 8;
    let fr = rect(body.x, by, body.w - 176, 24);
    ui.field(fr, &st.chat_line, "type here; Return sends; Command-V pastes");
    ui.outline(fr, t.accent);
    let mut entered = st.chat_line.feed(&ui.f);
    let paste = ui.button(rect(fr.right() + 8, by, 78, 24), "Paste", true) || ui.f.chord(crate::surface::Sym::Char('v'));
    if paste {
        for ch in console_clipboard().chars() {
            st.chat_line.text.insert(st.chat_line.caret, ch);
            st.chat_line.caret += ch.len_utf8();
        }
    }
    let line = st.chat_line.text.trim().to_string();
    entered |= ui.button(rect(fr.right() + 90, by, 78, 24), "Send", !line.is_empty());
    if entered && !line.is_empty() {
        st.ask = Some(Ask::Say(line));
        st.chat_line.clear();
        st.chat_at_end = true;
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"fleet":"grove","surveyed":"10:20:37",
        "ca":{"path":"x","fingerprint":"SHA256:abc","ok":true},
        "operator":{"principals":["fleet-operator","fleet-human"],"left_min":462},
        "machines":[{"name":"columbia","addr":"columbia","listener":"yes","os":"alpine","version":"3.24.3","cpus":"8","load":"7.9","idle":false,"treadcore":"37b6b65 2026-10-29","cert_days":89,"cert_ours":true},
                    {"name":"calber","addr":"calber","listener":"no","os":"ubuntu","version":"24.04","cert_days":-1,"cert_ours":false}],
        "buttons":[{"name":"Control","does":"the screen","ready":0,"total":2,"why":"no machine serves RDP yet"}]}"#;

    #[test]
    fn the_document_reads_as_the_authority_the_machines_and_the_buttons() {
        let m = Model::parse(DOC);
        assert_eq!(m.fleet, "grove");
        assert_eq!(m.ca_fingerprint, "SHA256:abc");
        assert_eq!(m.left_min, 462);
        assert_eq!(m.principals, ["fleet-operator", "fleet-human"]);
        assert_eq!(m.machines[0].cert_days, 89);
        assert!(m.machines[0].cert_ours);
        assert_eq!(m.machines[1].listener, "no");
        assert_eq!(m.buttons[0].why, "no machine serves RDP yet");
    }

    #[test]
    fn a_tool_that_did_not_answer_is_said_and_not_drawn_as_an_empty_fleet() {
        let m = Model::parse("no fleet file\nno nodes at ~/.copal");
        assert!(m.machines.is_empty());
        assert_eq!(m.why, "no nodes at ~/.copal");
    }

    #[test]
    fn a_selection_lights_its_machines_and_names_the_ones_nobody_could_ask() {
        let mut st = State::new(true);
        st.reply = r#"{"chosen":["columbia"],"unasked":["calber"]}"#.into();
        st.take_reply();
        assert_eq!(st.chosen, ["columbia"]);
        assert!(st.said.contains("calber"), "{}", st.said);
    }

    #[test]
    fn a_reply_is_taken_once_so_a_click_after_it_is_not_undone() {
        let mut st = State::new(true);
        st.reply = r#"{"chosen":["columbia"],"unasked":[]}"#.into();
        st.take_reply();
        st.chosen.clear();
        st.take_reply();
        assert!(st.chosen.is_empty(), "the old answer came back over the person's click");
    }

    #[test]
    fn a_verbs_answers_become_a_line_each() {
        let mut st = State::new(true);
        st.reply = r#"{"verb":"message hello","results":[{"node":"a","ok":true,"out":"kept"},{"node":"b","ok":false,"out":"refused"}]}"#.into();
        st.take_reply();
        assert_eq!(st.lines.len(), 2);
        assert!(!st.lines[1].1);
        assert_eq!(st.said, "message: 1 of 2 answered");
    }

    /// A press in one frame and its release in a later one, which is a person's
    /// click; the two in one frame is a fast click on a busy machine.
    fn press_then_release(x: i32, y: i32, together: bool) -> State {
        use crate::surface::{Button, Input};
        let m = Model::parse(DOC);
        let mut st = State::new(true);
        let mut ui_state = crate::ui::UiState::default();
        let down = Input::Button { x, y, button: Button::Left, down: true };
        let up = Input::Button { x, y, button: Button::Left, down: false };
        let frames: Vec<Vec<Input>> = if together { vec![vec![down, up]] } else { vec![vec![down], vec![up]] };
        for events in frames {
            let mut px = vec![0u8; 1200 * 760 * 4];
            let mut c = crate::draw::Canvas::new(&mut px, 1200, 760);
            let mut ui = Ui::begin(&mut c, crate::draw::LIGHT, &events, &mut ui_state);
            draw(&mut ui, &m, &mut st);
        }
        st
    }

    /// 30 October: Install worked on six machines and then "stopped". A press on it
    /// also landed in its row, the row is asked second and took the press, so unless
    /// press and release came in one frame the click lit the row and nothing else.
    #[test]
    fn install_is_pressed_however_long_the_click_takes() {
        for together in [true, false] {
            let found = (100..500).step_by(4).map(|y| press_then_release(800, y, together)).find(|st| st.ask.is_some());
            let st = found.expect("no y on that column pressed Install");
            assert_eq!(st.ask, Some(Ask::Install("calber".into())), "together: {}", together);
            assert!(st.chosen.is_empty(), "the press also lit the row (together: {})", together);
        }
    }

    #[test]
    fn a_row_is_still_lit_by_a_click_anywhere_else_on_it() {
        let lit = (100..500).step_by(4).map(|y| press_then_release(300, y, false)).find(|st| !st.chosen.is_empty());
        assert!(lit.is_some(), "no click lit a row");
    }

    #[test]
    fn the_same_answer_twice_is_still_an_answer() {
        let mut st = State::new(true);
        st.reply = r#"{"said":"every machine asked"}"#.into();
        st.take_reply();
        // A press clears the reply while it asks, so the same words read as new.
        st.said = "asking...".into();
        st.reply = String::new();
        st.take_reply();
        assert_eq!(st.said, "asking...", "an empty reply is nothing yet, not an answer");
        st.reply = r#"{"said":"every machine asked"}"#.into();
        st.take_reply();
        assert_eq!(st.said, "every machine asked");
    }

    #[test]
    fn the_room_reads_as_its_lines_and_who_can_be_reached() {
        let r = parse_room(r#"{"count":2,"lines":["09:50 orrery: hi","09:51 columbia: hello"],"machines":[{"name":"columbia","ok":true},{"name":"calber","ok":false}]}"#).unwrap();
        assert_eq!(r.lines[1], "09:51 columbia: hello");
        assert_eq!(r.machines[1], ("calber".to_string(), false));
        assert!(parse_room("no such thing").is_none());
    }

    #[test]
    fn a_long_line_wraps_and_an_empty_one_is_still_a_row() {
        assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn a_files_name_is_one_word_and_no_way_out_of_the_inbox() {
        assert!(good_file_name("notes-2.txt"));
        for bad in ["", ".profile", "../x", "a/b", "a b", "x;rm", &"y".repeat(65)] {
            assert!(!good_file_name(bad), "{:?} was allowed", bad);
        }
    }

    #[test]
    fn a_certificate_gone_reads_as_gone_rather_than_as_negative_minutes() {
        assert_eq!(left(-1), "expired or none");
        assert_eq!(left(125), "2 h 5 min left");
    }
}
