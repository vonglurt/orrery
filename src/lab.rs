//! The museum interface: a room of machines, and a row of verbs.
//!
//! The wall is eight nodes at a glance and the seat is one node you are working
//! on. This is the third thing, and it is what §IV-B of the lab report
//! described before either of the other two was built: a computer lab you
//! select from, with verbs that act on the selection.
//!
//! THE SELECTION IS THE ONLY MUTABLE STATE. Everything else on the screen is a
//! rendering of the last read model, and the read model is still the CLI's -- a
//! node id that is not in the document `copal fleet state` just returned cannot
//! be selected, because it was never drawn.
//!
//! THE ARITY OF THE SELECTION IS A FIRST-CLASS THING, NOT A DETAIL. Half the
//! verbs mean something different on one node than on eight, and two of them
//! mean nothing at all on eight. So the verb bar is rebuilt every frame, and a
//! verb that cannot apply is ABSENT RATHER THAN GREYED -- the same rule the web
//! console follows for the write route in the gallery posture, and for the same
//! reason: "the button is hidden" is not a security property, so the thing that
//! must not be reachable must not exist.

use crate::draw::{mix, Rgb, Theme};
use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::surface::Sym;
use crate::ui::{rect, Light, Rect, Ui};

// ------------------------------------------------------------- the read model ---

/// One machine, as the read model describes it. Never invented here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub id: String,
    pub address: String,
    pub role: String,
    /// DERIVED FROM, NEVER INSTEAD OF, these two.
    ///
    /// `wall.html:173` computes its own word from `announced` rather than
    /// reading `status`, and it is right to: the demo fixture says "down"
    /// where `copal-fleet-view`'s `assemble()` says "missing", and a face that
    /// keyed off the string would draw one of them wrong. The booleans are the
    /// facts; the word is a rendering.
    pub announced: bool,
    pub on_bus: bool,
    pub agent: String,
    pub scene: String,
    pub job: String,
    /// `None` is "not reported", and it is NOT zero. A node that never
    /// announced carries no temperature and the tile leaves the field blank
    /// rather than drawing a number nobody measured.
    pub temp_c: Option<i64>,
    pub tags: Vec<String>,
    pub last_seen: String,
    /// Which desktop this node is running, and therefore which remote-desktop
    /// protocol Control would speak to it. Empty until the node emits it --
    /// see copal-alpine-linux/docs/fleet-control.md §3.
    pub session: String,
    pub remote: Option<String>,
}

/// The three pictures the wall has always drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    /// Something answered for it.
    Up,
    /// It announced, and something above SSH is not talking.
    Quiet,
    /// It never announced. Not "down" -- nothing here has asked it.
    Absent,
}

impl Node {
    /// What this node is, from the facts rather than from the word.
    pub fn picture(&self) -> Picture {
        if !self.announced {
            return Picture::Absent;
        }
        // "An agent that died quietly is worse than no agent" -- the node
        // being up and the agent being up are two facts, never one.
        if !self.on_bus || self.agent == "silent" || self.agent == "unknown" {
            return Picture::Quiet;
        }
        Picture::Up
    }

    fn light(&self) -> (Light, bool) {
        match self.picture() {
            Picture::Up => (Light::Full, false),
            Picture::Quiet => (Light::Half, true),
            Picture::Absent => (Light::Hollow, false),
        }
    }

    /// The word on the tile's second line.
    pub fn word(&self) -> &str {
        match self.picture() {
            // A node that never announced has no temperature beside it, so
            // it gets the whole line and can afford the honest phrase.
            Picture::Absent => "not announced",
            // These two share the line with a reading, so they are the
            // beacon's own words rather than a sentence about them.
            Picture::Quiet if self.agent == "silent" => "silent",
            Picture::Quiet => "off bus",
            Picture::Up if self.role == "warden" => "warden",
            Picture::Up => "up",
        }
    }

    /// What the third line of the tile says, in the order the operator cares.
    fn subtitle(&self) -> &str {
        if self.picture() == Picture::Absent {
            return &self.last_seen;
        }
        if !self.job.is_empty() {
            return &self.job;
        }
        &self.scene
    }
}

/// A machine on the LAN that is not in this fleet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stranger {
    pub id: String,
    pub address: String,
}

/// The whole document, read once per refresh.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub fleet: String,
    pub warden: String,
    pub live: bool,
    pub why: String,
    pub declared: i64,
    pub announced: i64,
    pub on_bus: i64,
    pub nodes: Vec<Node>,
    /// Scene name to the ids running it, in the order the document listed them.
    pub scenes: Vec<(String, Vec<String>)>,
    pub tags: Vec<(String, usize)>,
    pub strangers: Vec<Stranger>,
    /// Set when the CLI answered with something that was not a fleet.
    pub error: String,
}

fn s_of(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

impl Model {
    /// Read the annotated document `fleet::annotate` produces.
    pub fn parse(doc: &str) -> Model {
        let v = match json::parse(doc) {
            Ok(v) => v,
            Err(e) => {
                return Model {
                    error: format!("unreadable JSON from the CLI: {}", e),
                    ..Default::default()
                }
            }
        };
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return Model { error: e.to_string(), ..Default::default() };
        }

        let mut m = Model {
            fleet: s_of(&v, "fleet"),
            warden: s_of(&v, "warden"),
            // `annotate` splices these in, and both halves always travel: a
            // fallback that is labelled but not explained is still a quiet
            // fallback.
            live: v.get("_live").and_then(|x| x.as_bool()).unwrap_or(false),
            why: s_of(&v, "_why"),
            ..Default::default()
        };
        if let Some(c) = v.get("counts") {
            m.declared = c.get("declared").and_then(|x| x.as_i64()).unwrap_or(0);
            m.announced = c.get("announced").and_then(|x| x.as_i64()).unwrap_or(0);
            m.on_bus = c.get("on_bus").and_then(|x| x.as_i64()).unwrap_or(0);
        }

        for n in v.get("nodes").and_then(|n| n.as_array()).unwrap_or(&[]) {
            let mut node = Node {
                id: s_of(n, "id"),
                address: s_of(n, "address"),
                role: s_of(n, "role"),
                announced: n.get("announced").and_then(|x| x.as_bool()).unwrap_or(false),
                on_bus: n.get("on_bus").and_then(|x| x.as_bool()).unwrap_or(false),
                agent: s_of(n, "agent"),
                scene: s_of(n, "scene"),
                job: s_of(n, "job"),
                temp_c: n.get("temp_c").and_then(|t| t.as_i64()),
                last_seen: s_of(n, "last_seen"),
                session: s_of(n, "session"),
                remote: n.get("remote").and_then(|r| r.as_str()).map(str::to_string),
                tags: Vec::new(),
            };
            for t in n.get("tags").and_then(|t| t.as_array()).unwrap_or(&[]) {
                if let Some(t) = t.as_str() {
                    node.tags.push(t.to_string());
                }
            }
            if node.id.is_empty() {
                continue;
            }
            m.nodes.push(node);
        }

        // Scenes come out of the document's own mapping rather than being
        // recomputed from the nodes: NEVER A GLOBAL BOOLEAN, and the CLI is
        // the one that decides what "running the show scene" means.
        if let json::Value::Obj(map) = v.get("scenes").unwrap_or(&json::Value::Null) {
            for (name, ids) in map {
                let mut who = Vec::new();
                for id in ids.as_array().unwrap_or(&[]) {
                    if let Some(id) = id.as_str() {
                        who.push(id.to_string());
                    }
                }
                m.scenes.push((name.clone(), who));
            }
        }

        // Tags are derived, because the document does not carry a tag index --
        // and a tag with no node under it is not a tag anyone can select.
        let mut tags: Vec<(String, usize)> = Vec::new();
        for n in &m.nodes {
            for t in &n.tags {
                match tags.iter_mut().find(|(name, _)| name == t) {
                    Some((_, c)) => *c += 1,
                    None => tags.push((t.clone(), 1)),
                }
            }
        }
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        m.tags = tags;

        for s in v.get("strangers").and_then(|s| s.as_array()).unwrap_or(&[]) {
            m.strangers.push(Stranger { id: s_of(s, "id"), address: s_of(s, "address") });
        }
        m
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }
}

// ----------------------------------------------------------------- the posture ---

/// A gallery screen and a phone on a lanyard are two audiences with different
/// rights, and so are a gallery screen and the operator's own Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    Gallery,
    Operator,
}

// ------------------------------------------------------------------ the verbs ---

/// A verb as the bar draws it.
///
/// `arity` is what the verb means about a selection, and it is the whole reason
/// this table exists separately from `verbs.rs`: that one is the security
/// boundary and says what may be RUN, this one says what may be OFFERED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// Aimed at exactly one node. Refused on a selection -- see §IV-C.
    One,
    /// Fans out, with a result per node.
    Many,
}

#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub name: &'static str,
    pub key: char,
    pub arity: Arity,
    /// False while the transport it needs does not exist. A verb in this state
    /// is NOT DRAWN -- the status line says why instead of a button lying.
    pub built: bool,
}

/// The lab report's table, and the state of each row in this build.
///
/// Terminal became `built: true` when `ssh.rs` was written, and Exchange and
/// Send when `sftp.rs` was -- that is what a phase looks like from in here,
/// rows of a table changing and a pane appearing that was not there before.
///
/// Observe, Control and Message are still `built: false`. The first two are
/// waiting for docs/wire.md's phases 7 and 8; Message is waiting for a
/// node-side verb that does not exist, which console.md explains and phase 10
/// is where it would be decided. They are listed rather than omitted so that the interface
/// can say what it is missing and why, which is the difference between a
/// console that is honest about being unfinished and one that looks finished.
pub const FACES: &[Face] = &[
    Face { name: "Observe",  key: 'o', arity: Arity::Many, built: false },
    Face { name: "Control",  key: 'c', arity: Arity::One,  built: false },
    Face { name: "Terminal", key: 't', arity: Arity::One,  built: true },
    Face { name: "Exchange", key: 'e', arity: Arity::One,  built: true },
    Face { name: "Send",     key: 's', arity: Arity::Many, built: true },
    Face { name: "Message",  key: 'm', arity: Arity::Many, built: false },
    Face { name: "Run",      key: 'r', arity: Arity::Many, built: true },
    Face { name: "Scene",    key: 'S', arity: Arity::Many, built: true },
    Face { name: "Snapshot", key: 'k', arity: Arity::Many, built: true },
    Face { name: "Power",    key: 'p', arity: Arity::Many, built: true },
];

/// Which verbs the bar draws for this posture and this selection.
///
/// THE GALLERY POSTURE RETURNS AN EMPTY LIST, and that is the native window's
/// version of the web console's missing route. There is no route here to leave
/// out, so what is left out is the dispatch table: a keystroke that would have
/// been a verb finds nothing to call.
pub fn visible(posture: Posture, selected: usize) -> Vec<&'static Face> {
    if posture == Posture::Gallery || selected == 0 {
        return Vec::new();
    }
    FACES
        .iter()
        .filter(|f| f.built)
        .filter(|f| match f.arity {
            Arity::One => selected == 1,
            Arity::Many => true,
        })
        .collect()
}

/// One node's answer to one verb.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub node: String,
    pub code: i32,
    pub output: String,
}

/// What a verb did, per node.
///
/// "I TOLD EIGHT MACHINES TO SHUT DOWN" AND "EIGHT MACHINES SHUT DOWN" ARE
/// DIFFERENT CLAIMS. That sentence is already in `fleet.rs`; this is where it
/// is drawn. A verb on eight nodes produces eight answers and the one thing
/// this must never do is collapse them into a success -- "six of eight got the
/// memo" is the normal case and the console has to be able to say so.
#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    pub verb: String,
    pub running: bool,
    pub items: Vec<Outcome>,
}

impl Results {
    pub fn ok(&self) -> usize {
        self.items.iter().filter(|o| o.code == 0).count()
    }
    pub fn failed(&self) -> usize {
        self.items.iter().filter(|o| o.code != 0).count()
    }
}

/// What the operator pressed, handed back for the caller to run.
///
/// `lab.rs` DRAWS; it does not shell out. The verb travels up to `main.rs`,
/// which owns the `Fleet`, for the same reason every screen here is a
/// rendering of a command a person could have typed.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Verb { name: &'static str, nodes: Vec<String> },
    /// Go to another face of the console.
    Go(nav::View),
    Quit,
}

// ------------------------------------------------------------------- the state ---

/// What survives between frames. The selection, and where the keyboard is.
#[derive(Debug, Default)]
pub struct Lab {
    pub selection: Vec<String>,
    /// Where a shift-click measures from.
    anchor: Option<usize>,
    /// The tile the arrow keys are on.
    pub focus: usize,
    pub posture: Posture,
    /// While this is set the results pane REPLACES the lab. One pane at a
    /// time, never a window inside the window, so there is never a question
    /// about what is on top.
    pub results: Option<Results>,
}

impl Default for Posture {
    fn default() -> Posture {
        Posture::Gallery
    }
}

impl Lab {
    pub fn new(posture: Posture) -> Lab {
        Lab { posture, ..Default::default() }
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.selection.iter().any(|s| s == id)
    }

    /// Drop any id the read model no longer carries.
    ///
    /// A selection is a set of names, and a name that has left the document has
    /// left the fleet -- keeping it would let a verb be aimed at a machine the
    /// certificate check no longer vouches for.
    pub fn reconcile(&mut self, m: &Model) {
        self.selection.retain(|id| m.index_of(id).is_some());
        if self.focus >= m.nodes.len() {
            self.focus = m.nodes.len().saturating_sub(1);
        }
    }

    fn select_only(&mut self, id: &str, at: usize) {
        self.selection = vec![id.to_string()];
        self.anchor = Some(at);
        self.focus = at;
    }

    fn toggle(&mut self, id: &str, at: usize) {
        match self.selection.iter().position(|s| s == id) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => self.selection.push(id.to_string()),
        }
        self.anchor = Some(at);
        self.focus = at;
    }

    fn extend_to(&mut self, m: &Model, at: usize) {
        let from = self.anchor.unwrap_or(at);
        let (lo, hi) = if from <= at { (from, at) } else { (at, from) };
        self.selection.clear();
        for n in &m.nodes[lo..=hi.min(m.nodes.len().saturating_sub(1))] {
            self.selection.push(n.id.clone());
        }
        self.focus = at;
    }

    /// Select a named group -- a scene, or a tag.
    pub fn select_group(&mut self, ids: &[String], m: &Model) {
        self.selection = ids
            .iter()
            .filter(|id| m.index_of(id).is_some())
            .cloned()
            .collect();
        self.anchor = self.selection.first().and_then(|id| m.index_of(id));
        if let Some(a) = self.anchor {
            self.focus = a;
        }
    }
}

// ---------------------------------------------------------------------- layout ---

const HEADER_H: i32 = 32;
const RAIL_W: i32 = 138;
const BAR_H: i32 = 32;
const STATUS_H: i32 = 22;
const PAD: i32 = 10;
const TILE_W: i32 = 152;
const TILE_H: i32 = 78;

pub struct Layout {
    pub header: Rect,
    pub rail: Rect,
    pub lab: Rect,
    pub bar: Rect,
    pub status: Rect,
}

pub fn layout(w: i32, h: i32) -> Layout {
    let all = rect(0, 0, w, h);
    let (header, rest) = all.cut_top(HEADER_H);
    let (status, rest) = rest.cut_bottom(STATUS_H);
    let (bar, rest) = rest.cut_bottom(BAR_H);
    let (rail, lab) = rest.cut_left(RAIL_W);
    Layout { header, rail, lab, bar, status }
}

/// Where each tile goes, in node order.
///
/// The column count is chosen so the tiles fill the row rather than leaving one
/// alone at the end of it -- a recurring element sitting in the same place on
/// each row is most of what makes a grid read as one object.
pub fn tile_rects(lab: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let inner = lab.inset(PAD);
    let cols = (((inner.w + PAD) / (TILE_W + PAD)).max(1) as usize).min(n);
    (0..n)
        .map(|i| {
            let (cx, cy) = (i % cols, i / cols);
            rect(
                inner.x + cx as i32 * (TILE_W + PAD),
                inner.y + cy as i32 * (TILE_H + PAD),
                TILE_W,
                TILE_H,
            )
        })
        .collect()
}

// --------------------------------------------------------------------- drawing ---

/// A temperature as a colour, `up` toward `alarm` across the range a board
/// actually lives in. The one thing flat colour cannot do is read as a quantity.
fn temp_colour(t: &Theme, c: i64) -> Rgb {
    let lo = 40i64;
    let hi = 75i64;
    let f = ((c - lo).clamp(0, hi - lo) * 255 / (hi - lo)) as u8;
    mix(t.up, t.alarm, f)
}

/// Draw the whole interface and return whatever the operator asked for.
pub fn draw(ui: &mut Ui, m: &Model, lab: &mut Lab) -> Option<Action> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let l = layout(w, h);
    let t = ui.t;
    let mut action = None;

    ui.panel(rect(0, 0, w, h), t.base);

    if !m.error.is_empty() {
        // Unreadable JSON is itself a picture the operator should get, rather
        // than a spinner.
        ui.label_in(PAD * 2, h / 2 - 20, w - PAD * 4, "the read model did not answer", &F10X20, t.alarm);
        ui.label_in(PAD * 2, h / 2 + 6, w - PAD * 4, &m.error, &F8X13, t.dim);
        return None;
    }

    if let Some(v) = draw_header(ui, &l, m) {
        action = Some(Action::Go(v));
    }
    draw_rail(ui, &l, m, lab);
    if lab.results.is_some() {
        if draw_results(ui, &l, lab) {
            lab.results = None;
        }
    } else {
        draw_lab(ui, &l, m, lab);
    }
    if let Some(a) = draw_bar(ui, &l, lab) {
        action = Some(a);
    }
    draw_status(ui, &l, m, lab);

    if let Some(a) = keys(ui, m, lab) {
        action = Some(a);
    }
    action
}

fn draw_header(ui: &mut Ui, l: &Layout, m: &Model) -> Option<nav::View> {
    let t = ui.t;
    ui.panel(l.header, t.panel);
    ui.hrule(l.header.x, l.header.bottom() - 1, l.header.w);

    let ty = l.header.y + (HEADER_H - F10X20.height as i32) / 2;
    let mut x = PAD;
    x += ui.label(x, ty, "orrery", &F10X20, t.ink);
    x += 14;

    // The navigation strip, in the same place on every view -- see nav.rs.
    let nav_at = rect(x, l.header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Lab);
    if let Some(v) = nav::chord(ui) {
        go = Some(v);
    }
    x += nav::width() + 14;
    ui.label(x, ty, &m.fleet, &F10X20, t.accent);

    // THE PICTURE AND ITS REASON ALWAYS TRAVEL TOGETHER. A fallback that is
    // labelled but not explained is still a quiet fallback -- the degradation
    // run of 2026-09-08 found exactly that.
    let sy = l.header.y + (HEADER_H - F8X13.height as i32) / 2;
    let counts = format!(
        "{} declared  {} up  {} bus",
        m.declared, m.announced, m.on_bus
    );
    // `text_right` hands back the x it drew AT, not the width it drew -- so
    // the next thing along is placed from that, and the badge cannot walk
    // backwards over the fleet name the way it did when this was read as a
    // width.
    let counts_x = ui.label_right(l.header.right() - PAD, sy, &counts, &F8X13, t.dim);

    let (word, light, colour) = if m.live {
        ("live".to_string(), Light::Full, t.up)
    } else if m.why.is_empty() {
        ("polled, not live".to_string(), Light::Hollow, t.warn)
    } else {
        // THE PICTURE AND ITS REASON TRAVEL TOGETHER. `bus off: timed out`
        // reports a missing component without telling the operator that the
        // tiles in front of them are four-minute-old beacons.
        (m.why.clone(), Light::Hollow, t.warn)
    };
    let word_x = ui.label_right(counts_x - 20, sy, &word, &F8X13, colour);
    ui.light(word_x - 9, l.header.y + HEADER_H / 2, 3, light, colour);
    go
}

fn draw_rail(ui: &mut Ui, l: &Layout, m: &Model, lab: &mut Lab) {
    let t = ui.t;
    ui.panel(l.rail, t.panel);
    ui.vrule(l.rail.right() - 1, l.rail.y, l.rail.h);

    let mut y = l.rail.y + 8;
    let head = |ui: &mut Ui, y: i32, s: &str| {
        ui.label(l.rail.x + 6, y, s, &F8X13, ui.t.accent);
    };

    head(ui, y, "SCENES");
    y += 18;
    for (name, who) in &m.scenes {
        let on = !who.is_empty() && who.iter().all(|id| lab.is_selected(id)) && lab.selection.len() == who.len();
        if ui.rail_item(rect(l.rail.x, y, RAIL_W - 1, 16), name, Some(who.len()), on) {
            lab.select_group(who, m);
        }
        y += 16;
    }

    if !m.tags.is_empty() {
        y += 10;
        head(ui, y, "TAGS");
        y += 18;
        for (name, n) in &m.tags {
            let ids: Vec<String> = m
                .nodes
                .iter()
                .filter(|nd| nd.tags.iter().any(|x| x == name))
                .map(|nd| nd.id.clone())
                .collect();
            let on = !ids.is_empty() && ids.iter().all(|id| lab.is_selected(id)) && lab.selection.len() == ids.len();
            if ui.rail_item(rect(l.rail.x, y, RAIL_W - 1, 16), name, Some(*n), on) {
                lab.select_group(&ids, m);
            }
            y += 16;
        }
    }

    // STRANGERS ARE A ROW, NOT A WIDGET. §IV-B's rule is that a machine on the
    // LAN which is not in this fleet is shown and never contacted, and that is
    // enforced by there being nothing here to click: no selection box, so no
    // verb can ever be aimed at one. The refusal is structural.
    if !m.strangers.is_empty() {
        y += 10;
        head(ui, y, "STRANGERS");
        y += 18;
        for s in &m.strangers {
            ui.label_in(l.rail.x + 6, y, RAIL_W - 12, &s.id, &F8X13, t.dim);
            y += 16;
        }
    }
}

fn draw_lab(ui: &mut Ui, l: &Layout, m: &Model, lab: &mut Lab) {
    let t = ui.t;
    let rects = tile_rects(l.lab, m.nodes.len());
    let mut clicked: Option<usize> = None;

    for (i, node) in m.nodes.iter().enumerate() {
        let r = rects[i];
        if r.bottom() > l.lab.bottom() {
            break;
        }
        let sel = lab.is_selected(&node.id);
        let hot = ui.is_hot(r);

        ui.panel(r, t.tile);
        if sel {
            ui.outline2(r, t.accent);
        } else {
            ui.outline(r, if hot { t.accent } else { t.line });
        }
        if i == lab.focus {
            ui.outline(r.inset(3), t.dim);
        }

        // line 1 -- the id
        ui.label_in(r.x + 8, r.y + 7, r.w - 16, &node.id, &F10X20, t.ink);

        // line 2 -- the light, the status, and the temperature if it was taken
        let (light, warn) = node.light();
        let lc = if light == Light::Hollow {
            t.dim
        } else if warn {
            t.warn
        } else {
            t.up
        };
        ui.light(r.x + 12, r.y + 36, 4, light, lc);
        let room = if node.temp_c.is_some() { r.w - 68 } else { r.w - 30 };
        ui.label_in(r.x + 22, r.y + 30, room, node.word(), &F8X13, t.ink);
        // "NOT REPORTED" AND "45" ARE DIFFERENT CLAIMS, so the field is left
        // blank rather than drawn as a zero.
        if let Some(c) = node.temp_c {
            ui.label_right(r.right() - 8, r.y + 30, &format!("{}C", c), &F8X13, temp_colour(&t, c));
        }

        // line 3 -- the scene, the job, or when it was last heard from
        ui.label_in(r.x + 8, r.y + 46, r.w - 16, node.subtitle(), &F8X13, t.dim);

        // line 4 -- the tags
        if !node.tags.is_empty() {
            ui.label_in(r.x + 8, r.y + 60, r.w - 16, &node.tags.join(" "), &F8X13, t.accent);
        }

        if ui.button_hit(r) {
            clicked = Some(i);
        }
    }

    if let Some(i) = clicked {
        let mods = ui.f.click_mods();
        let id = m.nodes[i].id.clone();
        if mods.shift {
            lab.extend_to(m, i);
        } else if mods.toggling() {
            lab.toggle(&id, i);
        } else {
            lab.select_only(&id, i);
        }
    }

    if !m.strangers.is_empty() {
        let y = l.lab.bottom() - 18;
        ui.hrule(l.lab.x + PAD, y, l.lab.w - PAD * 2);
        ui.label(
            l.lab.x + PAD,
            y + 4,
            "seen, never contacted",
            &F8X13,
            ui.t.dim,
        );
    }
}

/// The results pane. Returns true when the operator dismissed it.
fn draw_results(ui: &mut Ui, l: &Layout, lab: &Lab) -> bool {
    let t = ui.t;
    let r = lab.results.as_ref().expect("draw_results with no results");
    ui.panel(l.lab, t.base);

    let (head, rest) = l.lab.cut_top(28);
    ui.panel(head, t.panel);
    ui.hrule(head.x, head.bottom() - 1, head.w);

    let ty = head.y + (28 - F8X13.height as i32) / 2;
    let title = if r.running {
        format!("{}  running on {} ...", r.verb, r.items.len().max(1))
    } else {
        format!(
            "{}  {} of {} ok",
            r.verb,
            r.ok(),
            r.items.len()
        )
    };
    ui.label_in(head.x + PAD, ty, head.w - 120, &title, &F8X13, t.ink);
    let close = rect(head.right() - 78, head.y + 4, 68, 20);
    let dismissed = ui.button(close, "Dismiss", true);

    // A card per node. Never a single verdict.
    let inner = rest.inset(PAD);
    let cw = 236.min(inner.w);
    let cols = ((inner.w + PAD) / (cw + PAD)).max(1);
    for (i, o) in r.items.iter().enumerate() {
        let (x, y) = (i as i32 % cols, i as i32 / cols);
        let card = rect(
            inner.x + x * (cw + PAD),
            inner.y + y * (56 + PAD),
            cw,
            56,
        );
        if card.bottom() > inner.bottom() {
            break;
        }
        ui.panel(card, t.tile);
        ui.outline(card, t.line);
        // The exit code is the claim, so it is drawn as a colour as well as a
        // number -- a column of grey numbers does not say which row needs a
        // person.
        let (mark, colour) = if o.code == 0 {
            ("ok", t.up)
        } else {
            ("failed", t.alarm)
        };
        ui.c.rect(card.x, card.y, 3, card.h, colour);
        ui.label_in(card.x + 10, card.y + 8, cw - 80, &o.node, &F8X13, t.ink);
        ui.label_right(card.right() - 8, card.y + 8, mark, &F8X13, colour);
        let line = o.output.lines().next().unwrap_or("");
        ui.label_in(card.x + 10, card.y + 26, cw - 20, line, &F8X13, t.dim);
        if o.code != 0 {
            ui.label_right(
                card.right() - 8,
                card.y + 26,
                &format!("exit {}", o.code),
                &F8X13,
                t.dim,
            );
        }
    }
    dismissed
}

fn draw_bar(ui: &mut Ui, l: &Layout, lab: &Lab) -> Option<Action> {
    let t = ui.t;
    ui.panel(l.bar, t.panel);
    ui.hrule(l.bar.x, l.bar.y, l.bar.w);

    let faces = visible(lab.posture, lab.selection.len());
    let mut x = l.bar.x + PAD;
    let y = l.bar.y + 4;
    let mut action = None;
    for f in faces {
        let w = (F8X13.measure(f.name) as i32) + 20;
        if x + w > l.bar.right() - PAD {
            break;
        }
        if ui.button(rect(x, y, w, BAR_H - 8), f.name, true) {
            action = Some(Action::Verb { name: f.name, nodes: lab.selection.clone() });
        }
        x += w + 6;
    }
    action
}

fn draw_status(ui: &mut Ui, l: &Layout, m: &Model, lab: &Lab) {
    let t = ui.t;
    ui.panel(l.status, t.base);
    ui.hrule(l.status.x, l.status.y, l.status.w);
    let y = l.status.y + 4;

    let left = match lab.selection.len() {
        0 => "nothing selected".to_string(),
        1 => format!("1 selected  {}", lab.selection[0]),
        n => {
            let mut s = format!("{} selected  ", n);
            s.push_str(&lab.selection.join(" "));
            s
        }
    };
    ui.label_in(PAD, y, l.status.w - 240, &left, &F8X13, t.dim);

    // The unbuilt verbs say so here rather than appearing as buttons that lie.
    let missing: Vec<&str> = FACES.iter().filter(|f| !f.built).map(|f| f.name).collect();
    let right = match lab.posture {
        Posture::Gallery => "gallery (read-only)".to_string(),
        Posture::Operator => {
            if missing.is_empty() {
                "operator".to_string()
            } else {
                format!("operator  {} not built", missing.len())
            }
        }
    };
    ui.label_right(l.status.right() - PAD, y, &right, &F8X13, t.accent);
    let _ = m;
}

/// The keyboard. A gallery machine on a lectern may have a keyboard and no
/// pointer, and the arrow keys are how this gets tested without one.
fn keys(ui: &mut Ui, m: &Model, lab: &mut Lab) -> Option<Action> {
    if m.nodes.is_empty() {
        return None;
    }
    let last = m.nodes.len() - 1;
    let cols = {
        let (w, h) = (ui.c.w as i32, ui.c.h as i32);
        let inner = layout(w, h).lab.inset(PAD);
        ((inner.w + PAD) / (TILE_W + PAD)).max(1) as usize
    };

    if ui.f.pressed(Sym::Escape) {
        // The pane first. Escape means "put that away", and putting away the
        // selection underneath it in the same keystroke loses the thing the
        // operator is about to act on again.
        if lab.results.is_some() {
            lab.results = None;
        } else {
            lab.selection.clear();
        }
    }
    // Select all -- NEVER STRANGERS, which is guaranteed by there being no
    // stranger in `m.nodes` to begin with.
    if ui.f.chord(Sym::Char('a')) {
        lab.selection = m.nodes.iter().map(|n| n.id.clone()).collect();
    }
    if ui.f.pressed(Sym::Right) {
        lab.focus = (lab.focus + 1).min(last);
    }
    if ui.f.pressed(Sym::Left) {
        lab.focus = lab.focus.saturating_sub(1);
    }
    if ui.f.pressed(Sym::Down) {
        lab.focus = (lab.focus + cols).min(last);
    }
    if ui.f.pressed(Sym::Up) {
        lab.focus = lab.focus.saturating_sub(cols);
    }
    if ui.f.pressed(Sym::Space) {
        let id = m.nodes[lab.focus].id.clone();
        let at = lab.focus;
        lab.toggle(&id, at);
    }
    if ui.f.pressed(Sym::Return) && !lab.selection.is_empty() {
        let id = m.nodes[lab.focus].id.clone();
        let at = lab.focus;
        lab.select_only(&id, at);
    }

    // A verb's keystroke, but only where the bar would have drawn its button.
    // The dispatch table and the bar are the same list, so a key cannot reach
    // a verb the posture does not offer.
    for f in visible(lab.posture, lab.selection.len()) {
        if ui.f.pressed(Sym::Char(f.key)) {
            return Some(Action::Verb { name: f.name, nodes: lab.selection.clone() });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;
    use crate::fleet::{annotate, demo_doc};
    use crate::surface::{Button, Input, Mods};
    use crate::ui::UiState;

    fn model() -> Model {
        Model::parse(&annotate(&demo_doc()))
    }

    fn run<R>(lab: &mut Lab, m: &Model, events: &[Input], f: impl FnOnce(&mut Ui, &Model, &mut Lab) -> R) -> R {
        let mut px = vec![0u8; 960 * 600 * 4];
        let mut c = draw::Canvas::new(&mut px, 960, 600);
        let mut st = UiState::default();
        let mut ui = Ui::begin(&mut c, draw::LIGHT, events, &mut st);
        f(&mut ui, m, lab)
    }

    fn click(x: i32, y: i32, mods: Mods) -> Vec<Input> {
        vec![
            Input::Key { scancode: 0, sym: Sym::Unknown, down: true, mods },
            Input::Button { x, y, button: Button::Left, down: true },
            Input::Button { x, y, button: Button::Left, down: false },
        ]
    }

    #[test]
    fn the_fixture_parses_into_the_museums_eight() {
        let m = model();
        assert_eq!(m.fleet, "museum");
        assert_eq!(m.nodes.len(), 8);
        assert_eq!(m.warden, "museum-06");
        assert_eq!(m.declared, 8);
        assert!(m.live);
        assert_eq!(m.strangers.len(), 1);
        assert_eq!(m.strangers[0].id, "epson-XY10");
    }

    #[test]
    fn a_node_that_never_announced_carries_no_temperature() {
        let m = model();
        let seven = m.nodes.iter().find(|n| n.id == "museum-07").unwrap();
        assert_eq!(seven.temp_c, None, "a missing node was given a temperature");
        // NOT keyed off `status`: the fixture says "down" and the real CLI
        // says "missing", and the page reads neither.
        assert!(!seven.announced);
        assert_eq!(seven.picture(), Picture::Absent);
        assert_eq!(seven.word(), "not announced");
        // It has no reading beside it, so the honest phrase fits the line.
        assert!(
            (F8X13.measure(seven.word()) as i32) <= TILE_W - 30,
            "the absent node's word does not fit its tile"
        );
        assert_eq!(seven.light().0, Light::Hollow);
        // And the subtitle falls back to when it was last heard from.
        assert_eq!(seven.subtitle(), "08:12");
    }

    #[test]
    fn a_quiet_agent_is_told_apart_from_a_missing_machine() {
        let m = model();
        let eight = m.nodes.iter().find(|n| n.id == "museum-08").unwrap();
        assert_eq!(eight.agent, "silent");
        assert_eq!(eight.picture(), Picture::Quiet);
        assert_eq!(eight.word(), "silent");
        // This one SHARES the line with a temperature, so it has to fit the
        // smaller room -- which is the bug the first render showed.
        for n in &m.nodes {
            let room = if n.temp_c.is_some() { TILE_W - 68 } else { TILE_W - 30 };
            assert!(
                (F8X13.measure(n.word()) as i32) <= room,
                "{}: the word {:?} is wider than its tile",
                n.id,
                n.word()
            );
        }
        assert_eq!(eight.light(), (Light::Half, true));
        let one = m.nodes.iter().find(|n| n.id == "museum-01").unwrap();
        assert_eq!(one.picture(), Picture::Up);
        assert_eq!(one.light(), (Light::Full, false));
        // The warden says so rather than saying "up" like the rest.
        let six = m.nodes.iter().find(|n| n.id == "museum-06").unwrap();
        assert_eq!(six.word(), "warden");
    }

    #[test]
    fn the_scenes_come_from_the_document_and_the_tags_are_derived() {
        let m = model();
        let show = m.scenes.iter().find(|(n, _)| n == "show").unwrap();
        assert_eq!(show.1.len(), 5);
        let north = m.tags.iter().find(|(n, _)| n == "north").unwrap();
        assert_eq!(north.1, 3);
        // A tag with no node under it is not a tag anyone can select.
        assert!(m.tags.iter().all(|(_, c)| *c > 0));
    }

    #[test]
    fn unreadable_json_becomes_a_picture_of_the_failure() {
        let m = Model::parse("not json at all");
        assert!(!m.error.is_empty());
        assert!(m.nodes.is_empty());
    }

    #[test]
    fn the_gallery_posture_offers_no_verb_at_all() {
        // The native window's version of the web console's missing route.
        assert!(visible(Posture::Gallery, 1).is_empty());
        assert!(visible(Posture::Gallery, 8).is_empty());
    }

    #[test]
    fn control_is_never_offered_on_a_selection() {
        // §IV-C: broadcasting keystrokes to eight machines produces divergent
        // state nobody can see. The button is not drawn, not greyed.
        let many: Vec<&str> = visible(Posture::Operator, 3).iter().map(|f| f.name).collect();
        for one_only in ["Control", "Terminal", "Exchange"] {
            assert!(!many.contains(&one_only), "{} was offered on a selection", one_only);
        }
        assert!(many.contains(&"Run"));
        assert!(many.contains(&"Scene"));
    }

    #[test]
    fn nothing_is_offered_for_an_empty_selection() {
        assert!(visible(Posture::Operator, 0).is_empty());
    }

    #[test]
    fn an_unbuilt_verb_is_absent_rather_than_drawn() {
        let names: Vec<&str> = visible(Posture::Operator, 1).iter().map(|f| f.name).collect();
        for unbuilt in ["Observe", "Control", "Message"] {
            assert!(!names.contains(&unbuilt), "{} was offered with no transport", unbuilt);
        }
        // And the ones phases 5 and 6 built. These assertions are the
        // difference those phases made, stated where a later phase will have
        // to come and change them again.
        for built in ["Terminal", "Exchange"] {
            assert!(
                names.contains(&built),
                "{} has a transport now and is still not offered",
                built
            );
        }
        assert!(
            visible(Posture::Operator, 3).iter().any(|f| f.name == "Send"),
            "Send is the verb for several nodes and was not offered for several"
        );
    }

    #[test]
    fn clicking_a_tile_selects_exactly_it() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        let r = tile_rects(layout(960, 600).lab, 8)[2];
        run(&mut lab, &m, &click(r.x + 20, r.y + 20, Mods::default()), |ui, m, lab| {
            draw(ui, m, lab)
        });
        assert_eq!(lab.selection, vec!["museum-03".to_string()]);
    }

    #[test]
    fn the_command_chord_adds_to_the_selection_and_takes_away_again() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        let rects = tile_rects(layout(960, 600).lab, 8);
        let toggling = Mods { logo: true, ..Default::default() };
        run(&mut lab, &m, &click(rects[0].x + 20, rects[0].y + 20, Mods::default()), |ui, m, lab| draw(ui, m, lab));
        run(&mut lab, &m, &click(rects[3].x + 20, rects[3].y + 20, toggling), |ui, m, lab| draw(ui, m, lab));
        assert_eq!(lab.selection, vec!["museum-01".to_string(), "museum-04".to_string()]);
        run(&mut lab, &m, &click(rects[3].x + 20, rects[3].y + 20, toggling), |ui, m, lab| draw(ui, m, lab));
        assert_eq!(lab.selection, vec!["museum-01".to_string()]);
    }

    #[test]
    fn shift_extends_the_selection_across_the_range() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        let rects = tile_rects(layout(960, 600).lab, 8);
        let shift = Mods { shift: true, ..Default::default() };
        run(&mut lab, &m, &click(rects[1].x + 20, rects[1].y + 20, Mods::default()), |ui, m, lab| draw(ui, m, lab));
        run(&mut lab, &m, &click(rects[4].x + 20, rects[4].y + 20, shift), |ui, m, lab| draw(ui, m, lab));
        assert_eq!(
            lab.selection,
            vec![
                "museum-02".to_string(),
                "museum-03".to_string(),
                "museum-04".to_string(),
                "museum-05".to_string()
            ]
        );
    }

    #[test]
    fn clicking_a_scene_selects_the_nodes_running_it() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        // The rail's first scene row. Scenes are ordered by the document's map.
        let first = m.scenes[0].clone();
        let y = layout(960, 600).rail.y + 8 + 18;
        run(&mut lab, &m, &click(40, y + 4, Mods::default()), |ui, m, lab| draw(ui, m, lab));
        assert_eq!(lab.selection.len(), first.1.len());
        assert_eq!(lab.selection, first.1);
    }

    #[test]
    fn a_stranger_cannot_be_selected_because_it_is_not_a_tile() {
        let m = model();
        assert!(m.index_of("epson-XY10").is_none());
        let mut lab = Lab::new(Posture::Operator);
        lab.select_group(&["epson-XY10".to_string()], &m);
        assert!(lab.selection.is_empty(), "a stranger reached the selection");
    }

    #[test]
    fn select_all_takes_the_fleet_and_not_the_strangers() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        let ev = [Input::Key {
            scancode: 0x1e,
            sym: Sym::Char('a'),
            down: true,
            mods: Mods { logo: true, ..Default::default() },
        }];
        run(&mut lab, &m, &ev, |ui, m, lab| draw(ui, m, lab));
        assert_eq!(lab.selection.len(), 8);
        assert!(!lab.selection.iter().any(|s| s == "epson-XY10"));
    }

    #[test]
    fn escape_clears_the_selection() {
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        lab.selection = vec!["museum-01".into()];
        let ev = [Input::Key { scancode: 1, sym: Sym::Escape, down: true, mods: Mods::default() }];
        run(&mut lab, &m, &ev, |ui, m, lab| draw(ui, m, lab));
        assert!(lab.selection.is_empty());
    }

    #[test]
    fn a_verbs_keystroke_cannot_reach_a_verb_the_posture_does_not_offer() {
        let m = model();
        let mut lab = Lab::new(Posture::Gallery);
        lab.selection = vec!["museum-01".into()];
        let ev = [Input::Key { scancode: 0x13, sym: Sym::Char('r'), down: true, mods: Mods::default() }];
        let got = run(&mut lab, &m, &ev, |ui, m, lab| draw(ui, m, lab));
        assert_eq!(got, None, "a gallery screen ran a verb from the keyboard");

        lab.posture = Posture::Operator;
        let got = run(&mut lab, &m, &ev, |ui, m, lab| draw(ui, m, lab));
        assert_eq!(
            got,
            Some(Action::Verb { name: "Run", nodes: vec!["museum-01".into()] })
        );
    }

    #[test]
    fn a_selection_survives_a_refresh_but_a_departed_node_does_not() {
        let mut m = model();
        let mut lab = Lab::new(Posture::Operator);
        lab.selection = vec!["museum-01".into(), "museum-07".into()];
        lab.reconcile(&m);
        assert_eq!(lab.selection.len(), 2, "a live selection was dropped");

        // museum-07 leaves the fleet between two refreshes.
        m.nodes.retain(|n| n.id != "museum-07");
        lab.reconcile(&m);
        assert_eq!(lab.selection, vec!["museum-01".to_string()]);
    }

    #[test]
    fn the_layout_gives_every_region_a_place_and_loses_no_pixels() {
        let l = layout(960, 600);
        assert_eq!(l.header.h, HEADER_H);
        assert_eq!(l.status.bottom(), 600);
        assert_eq!(l.bar.bottom(), l.status.y);
        assert_eq!(l.rail.right(), l.lab.x);
        assert_eq!(l.header.h + l.lab.h + l.bar.h + l.status.h, 600);
    }

    #[test]
    fn the_tiles_form_a_grid_that_fills_its_rows() {
        let l = layout(960, 600);
        let r = tile_rects(l.lab, 8);
        assert_eq!(r.len(), 8);
        // Row one and row two start at the same x.
        let cols = r.iter().filter(|t| t.y == r[0].y).count();
        assert!(cols >= 2, "the lab drew one tile per row at 960px");
        assert_eq!(r[0].x, r[cols].x, "the grid did not line up");
        assert!(r[cols].y > r[0].y);
        // Nothing escapes the lab.
        for t in &r {
            assert!(t.x >= l.lab.x && t.right() <= l.lab.right(), "a tile left the lab");
        }
        assert!(tile_rects(l.lab, 0).is_empty());
    }

    #[test]
    fn a_narrow_window_still_lays_out_rather_than_dividing_by_zero() {
        let l = layout(320, 400);
        let r = tile_rects(l.lab, 8);
        assert_eq!(r.len(), 8);
    }

    #[test]
    fn drawing_the_whole_interface_touches_no_pixel_twice_wrongly() {
        // The regression this is really for: draw() panicking on an edge case
        // in the fixture. It renders every node, every scene, every tag and
        // the stranger row.
        let m = model();
        let mut lab = Lab::new(Posture::Operator);
        lab.selection = vec!["museum-01".into(), "museum-03".into()];
        for (w, h) in [(960, 600), (640, 480), (1280, 800), (420, 380)] {
            let mut px = vec![0u8; w * h * 4];
            let mut c = draw::Canvas::new(&mut px, w, h);
            let mut st = UiState::default();
            let mut ui = Ui::begin(&mut c, draw::DARK, &[], &mut st);
            draw(&mut ui, &m, &mut lab);
        }
    }
}
