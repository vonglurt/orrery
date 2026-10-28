//! Training: what the fleet is teaching, and which machines are doing it.
//!
//! The Lab shows what each machine IS. This face answers the two questions a
//! person standing in front of the fleet asks while it trains a net: what is
//! it doing right now, and is the net getting better? It is laid out to be
//! read, not decoded:
//!
//!   - one sentence on top that says what is happening, and the round's three
//!     steps (record, train, judge) with the current one lit;
//!   - on the left, a card a machine: what it is for, how busy it is (its
//!     load against its cores, a GPU's use), and what it is doing now, in
//!     words, with the command doing it;
//!   - on the right, the net itself: its layers drawn as the weight matrices
//!     they are, the 47 scores split into the heads they belong to, how often
//!     it chooses what the scripted brain chose, and where it stands in the
//!     census against the official net.
//!
//! THE SAME RULE AS EVERY OTHER FACE: the whole picture is the document one
//! command prints (`--training-cmd`; treadcore's is `tools/plans/treadcore-view.py`),
//! polled on a thread. Nothing is discovered here, and there are no verbs.

use crate::draw::{mix, Rgb};
use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::surface::Sym;
use crate::ui::{rect, Light, Rect, Ui};

const HEADER_H: i32 = 32;
const PAD: i32 = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub busy_pct: i64,
    pub mem_mb: i64,
    pub mem_total_mb: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub address: String,
    pub roles: Vec<String>,
    pub up: bool,
    pub cores: i64,
    pub load: f64,
    pub mem_mb: i64,
    pub avail_mb: i64,
    pub gpu: Option<Gpu>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lane {
    pub host: String,
    pub jobs: i64,
    pub state: String,
    pub job: String,
    pub age_s: Option<i64>,
    pub pipe: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub name: String,
    /// The address shards are gathered at (the GPU node).
    pub store: String,
    pub done: i64,
    pub total: i64,
    pub running: i64,
    pub seeds: i64,
    pub ended: bool,
    pub lanes: Vec<Lane>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    pub inputs: i64,
    pub outputs: i64,
    pub max_abs: f64,
    /// Rows are outputs, columns inputs, shrunk for drawing.
    pub cells: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Net {
    pub name: String,
    pub sizes: Vec<i64>,
    pub parameters: i64,
    pub heads: Vec<(String, i64)>,
    pub layers: Vec<Layer>,
    /// What the inputs and outputs are, by group: (name, first, last).
    pub input_groups: Vec<(String, i64, i64)>,
    pub output_groups: Vec<(String, i64, i64)>,
}

/// What the view remembers between frames: which matrix is enlarged, whether
/// it may steer the queue (operator posture only: a button that must not be
/// pressable is not drawn), and the control a button asked for, which the
/// loop runs and clears.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub zoom: Option<usize>,
    pub operator: bool,
    pub ctl: Option<String>,
    /// What the last control said, shown until the next.
    pub said: String,
    /// Shut the fleet down was pressed once: the next press does it.
    pub arm_shutdown: bool,
    /// Which page of the queue's view: 0 Fleet, 1 Runs, 2 Found, 3 Net.
    pub page: usize,
}

/// One machine's lane in the queue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QLane {
    pub host: String,
    pub state: String,
    pub kind: String,
    pub title: String,
    pub age_s: Option<i64>,
}

/// The fleet-queue (orrery's tools/fleet-queue.py) as its status said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    pub name: String,
    pub state: String,
    pub adding: bool,
    /// kind -> (queued, running, done, failed, stopped)
    pub counts: Vec<(String, [i64; 5])>,
    pub lanes: Vec<QLane>,
    /// (kind, title) of the next few.
    pub upcoming: Vec<(String, String)>,
    /// (kind, title, ok) of the last few.
    pub recent: Vec<(String, String, bool)>,
    /// The shutdown report's one line, once the fleet is shut down.
    pub shutdown: String,
    /// The last sync: when, and how many censuses the fleet's ledger holds.
    pub synced: String,
    /// The newest coefficients the fleet changed, oldest first.
    pub ticker: Vec<Tick>,
    /// Found Improvement, by variable: (name, times).
    pub lead_vars: Vec<(String, i64)>,
    /// By machine: (name, censuses read, improvements found).
    pub lead_hosts: Vec<(String, i64, i64)>,
    /// The runs a person asked for.
    pub runs: Vec<RunRow>,
    /// Push permission to the project's branch: (ok, "url branch", detail).
    /// Approve is a push, so a template must have it.
    pub push: Option<(bool, String, String)>,
}

/// One coefficient one census moved: what, to what, how far from its
/// default, what the census read, and which machine read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub t: String,
    pub kind: String,
    pub var: String,
    pub value: Option<f64>,
    pub offset: Option<f64>,
    pub d: Option<f64>,
    pub se: Option<f64>,
    pub host: String,
    pub improved: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub detail: String,
}

/// (head, the net's agreement, bc1's, always-the-commonest's), percent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quality {
    pub net: String,
    pub heads: Vec<(String, f64, f64, f64)>,
    pub census: Option<(f64, f64, f64, f64, f64, f64)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub why: String,
    pub fleet: String,
    pub project: String,
    pub machines: Vec<Machine>,
    pub run: Option<Run>,
    pub round: i64,
    pub stages: Vec<(String, String)>,
    pub looping: bool,
    pub best: String,
    pub net: Option<Net>,
    pub quality: Option<Quality>,
    pub queue: Option<Queue>,
}

fn s(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0)
}
fn arr<'a>(v: &'a json::Value, k: &str) -> &'a [json::Value] {
    v.get(k).and_then(|x| x.as_array()).unwrap_or(&[])
}

impl Model {
    pub fn parse(text: &str, source: &str) -> Model {
        let mut m = Model::default();
        if text.trim().is_empty() {
            m.why = if source.is_empty() {
                "Nothing to show yet: start orrery with --queue DIR (a fleet queue), \
                 or --training-cmd CMD"
                    .into()
            } else {
                format!("Asking the fleet what it is doing: {}", source)
            };
            return m;
        }
        let v = match json::parse(text) {
            Ok(v) => v,
            Err(e) => {
                m.why = format!("The training command did not print JSON: {}", e.0);
                return m;
            }
        };
        if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
            m.why = format!("The training command failed: {}", e);
            return m;
        }
        m.fleet = s(&v, "fleet");
        m.project = s(&v, "project");
        for x in arr(&v, "machines") {
            m.machines.push(Machine {
                name: s(x, "name"),
                address: s(x, "address"),
                roles: arr(x, "roles").iter().filter_map(|r| r.as_str().map(str::to_string)).collect(),
                up: x.get("up").and_then(|b| b.as_bool()).unwrap_or(false),
                cores: n(x, "cores") as i64,
                load: n(x, "load"),
                mem_mb: n(x, "mem_mb") as i64,
                avail_mb: n(x, "avail_mb") as i64,
                gpu: x.get("gpu").filter(|g| g.get("name").is_some()).map(|g| Gpu {
                    name: s(g, "name"),
                    busy_pct: n(g, "busy_pct") as i64,
                    mem_mb: n(g, "mem_mb") as i64,
                    mem_total_mb: n(g, "mem_total_mb") as i64,
                }),
            });
        }
        if let Some(r) = v.get("run").filter(|r| r.get("run").is_some()) {
            m.run = Some(Run {
                name: s(r, "run"),
                store: s(r, "store").rsplit('@').next().unwrap_or("").to_string(),
                done: n(r, "done") as i64,
                total: n(r, "total") as i64,
                running: n(r, "running") as i64,
                seeds: n(r, "seeds") as i64,
                ended: r.get("ended").and_then(|b| b.as_bool()).unwrap_or(false),
                lanes: arr(r, "lanes")
                    .iter()
                    .map(|l| Lane {
                        host: s(l, "host"),
                        jobs: n(l, "jobs") as i64,
                        state: s(l, "state"),
                        job: s(l, "job"),
                        age_s: l.get("age_s").and_then(|a| a.as_num()).map(|a| a as i64),
                        pipe: s(l, "pipe"),
                    })
                    .collect(),
            });
        }
        if let Some(l) = v.get("loop") {
            m.round = n(l, "round") as i64;
            m.looping = l.get("running").and_then(|b| b.as_bool()).unwrap_or(false);
            m.best = s(l, "best");
            m.stages = arr(l, "stages").iter().map(|x| (s(x, "stage"), s(x, "state"))).collect();
        }
        if let Some(q) = v.get("queue").filter(|q| q.get("state").is_some()) {
            let mut counts = Vec::new();
            if let Some(json::Value::Obj(map)) = q.get("counts") {
                for (k, c) in map {
                    let g = |s: &str| c.get(s).and_then(|x| x.as_num()).unwrap_or(0.0) as i64;
                    counts.push((k.clone(), [g("queue"), g("running"), g("done"), g("failed"), g("stopped")]));
                }
            }
            m.queue = Some(Queue {
                name: s(q, "name"),
                state: s(q, "state"),
                adding: q.get("adding").and_then(|b| b.as_bool()).unwrap_or(false),
                counts,
                lanes: arr(q, "lanes")
                    .iter()
                    .map(|l| QLane {
                        host: s(l, "host"),
                        state: s(l, "state"),
                        kind: s(l, "kind"),
                        title: s(l, "title"),
                        age_s: l.get("age_s").and_then(|a| a.as_num()).map(|a| a as i64),
                    })
                    .collect(),
                upcoming: arr(q, "upcoming").iter().map(|j| (s(j, "kind"), s(j, "title"))).collect(),
                recent: arr(q, "recent")
                    .iter()
                    .map(|j| (s(j, "kind"), s(j, "title"), j.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)))
                    .collect(),
                shutdown: q.get("shutdown").map(|r| s(r, "summary")).unwrap_or_default(),
                ticker: arr(q, "ticker")
                    .iter()
                    .map(|e| Tick {
                        t: s(e, "t"),
                        kind: s(e, "kind"),
                        var: s(e, "var"),
                        value: e.get("value").and_then(|x| x.as_num()),
                        offset: e.get("offset").and_then(|x| x.as_num()),
                        d: e.get("d").and_then(|x| x.as_num()),
                        se: e.get("se").and_then(|x| x.as_num()),
                        host: s(e, "host"),
                        improved: e.get("improved").and_then(|b| b.as_bool()).unwrap_or(false),
                    })
                    .collect(),
                lead_vars: arr(q, "lead_vars")
                    .iter()
                    .filter_map(|p| match p {
                        json::Value::Arr(v) if v.len() == 2 => Some((v[0].as_str().unwrap_or("").to_string(), v[1].as_num().unwrap_or(0.0) as i64)),
                        _ => None,
                    })
                    .collect(),
                lead_hosts: arr(q, "lead_hosts")
                    .iter()
                    .filter_map(|p| match p {
                        json::Value::Arr(v) if v.len() == 3 => Some((
                            v[0].as_str().unwrap_or("").to_string(),
                            v[1].as_num().unwrap_or(0.0) as i64,
                            v[2].as_num().unwrap_or(0.0) as i64,
                        )),
                        _ => None,
                    })
                    .collect(),
                push: q.get("push").filter(|p| p.get("ok").is_some()).map(|p| {
                    (p.get("ok").and_then(|b| b.as_bool()).unwrap_or(false), format!("{} {}", s(p, "url"), s(p, "branch")), s(p, "detail"))
                }),
                runs: arr(q, "runs")
                    .iter()
                    .map(|r| RunRow { id: s(r, "id"), kind: s(r, "kind"), status: s(r, "status"), detail: if s(r, "why").is_empty() { s(r, "detail") } else { s(r, "why") } })
                    .collect(),
                synced: q
                    .get("sync")
                    .filter(|x| x.get("t").is_some())
                    .map(|x| format!("synced {}: {} censuses in the fleet's ledger", s(x, "t").replace('T', " "), n(x, "rows") as i64))
                    .unwrap_or_default(),
            });
        }
        if let Some(x) = v.get("net").filter(|x| x.get("sizes").is_some()) {
            m.net = Some(Net {
                name: s(x, "name"),
                sizes: arr(x, "sizes").iter().filter_map(|v| v.as_num()).map(|v| v as i64).collect(),
                parameters: n(x, "parameters") as i64,
                heads: arr(x, "heads").iter().map(|h| (s(h, "name"), n(h, "choices") as i64)).collect(),
                input_groups: arr(x, "input_groups").iter().map(|g| (s(g, "name"), n(g, "from") as i64, n(g, "to") as i64)).collect(),
                output_groups: arr(x, "output_groups").iter().map(|g| (s(g, "name"), n(g, "from") as i64, n(g, "to") as i64)).collect(),
                layers: arr(x, "layers")
                    .iter()
                    .map(|l| Layer {
                        inputs: n(l, "inputs") as i64,
                        outputs: n(l, "outputs") as i64,
                        max_abs: n(l, "max_abs"),
                        cells: arr(l, "cells")
                            .iter()
                            .map(|row| row.as_array().unwrap_or(&[]).iter().filter_map(|c| c.as_num()).collect())
                            .collect(),
                    })
                    .collect(),
            });
        }
        if let Some(q) = v.get("quality") {
            let get = |part: &str, head: &str| -> f64 {
                q.get(part).and_then(|p| p.get(head)).and_then(|x| x.as_num()).unwrap_or(f64::NAN)
            };
            let mut heads = Vec::new();
            for h in ["drive", "gun", "mortar", "drone", "lead"] {
                heads.push((h.to_string(), get("held_out", h), get("bc1", h), get("commonest", h)));
            }
            let census = q.get("census").map(|c| {
                (n(c, "best"), n(c, "best_se"), n(c, "official"), n(c, "official_se"), n(c, "vs_official"), n(c, "vs_official_se"))
            });
            m.quality = Some(Quality { net: s(q, "net"), heads, census });
        }
        m
    }
}

fn ago(s: i64) -> String {
    if s < 90 {
        format!("{} s", s)
    } else if s < 5400 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

/// What a lane is doing, in words. `on_store`: this machine is where the
/// shards are gathered, so "sending" is filing them on itself.
fn doing(l: &Lane, on_store: bool) -> String {
    let when = l.age_s.map(|a| format!(", for {}", ago(a))).unwrap_or_default();
    match l.state.as_str() {
        "recording" => format!("Recording matches from seed {}{}", l.job, when),
        "sending" if on_store => format!("Filing matches {} and checking them{}", l.job, when),
        "sending" => format!("Sending matches {} to the GPU machine{}", l.job, when),
        "finished" => format!("Finished its share: {} jobs", l.jobs),
        "waiting to start" => "Waiting to start".into(),
        other => other.to_string(),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A horizontal bar: `frac` of it filled.
fn bar(ui: &mut Ui, r: Rect, frac: f64, c: Rgb) {
    let t = ui.t;
    ui.panel(r, t.base);
    ui.outline(r, t.line);
    let w = ((r.w - 2) as f64 * frac.clamp(0.0, 1.0)).round() as i32;
    ui.c.rect(r.x + 1, r.y + 1, w, r.h - 2, c);
}

pub fn draw(ui: &mut Ui, m: &Model, st: &mut State) -> Option<nav::View> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let t = ui.t;
    ui.panel(rect(0, 0, w, h), t.base);
    let (header, rest) = rect(0, 0, w, h).cut_top(HEADER_H);

    // ---- header -----------------------------------------------------------
    ui.panel(header, t.panel);
    ui.hrule(header.x, header.bottom() - 1, header.w);
    let ty = header.y + (HEADER_H - F10X20.height as i32) / 2;
    ui.label(PAD, ty, "orrery", &F10X20, t.ink);
    let nav_at = rect(PAD + F10X20.measure("orrery") as i32 + 14, header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Training);
    if let Some(v) = nav::chord(ui) {
        go = Some(v);
    }
    let sy = header.y + (HEADER_H - F8X13.height as i32) / 2;
    if !m.fleet.is_empty() {
        let up = m.machines.iter().filter(|x| x.up).count();
        let right = format!("fleet {}  {} of {} machines answering", m.fleet, up, m.machines.len());
        ui.label_right(header.right() - PAD, sy, &right, &F8X13, t.dim);
    }

    let body = rest.inset(PAD);
    if !m.why.is_empty() {
        ui.label_in(body.x, body.y + 8, body.w, &m.why, &F8X13, t.warn);
        return go;
    }

    // ---- what is happening, in a sentence, and the three steps ------------
    let mut y = body.y;
    // An enlarged matrix takes the whole body; Escape or Close puts it away.
    if let (Some(i), Some(net)) = (st.zoom, &m.net) {
        if i < net.layers.len() {
            if ui.f.pressed(Sym::Escape) || draw_zoom(ui, body, net, i) {
                st.zoom = None;
            }
            return go;
        }
        st.zoom = None;
    }
    let (sentence, step) = headline(m);
    ui.label_in(body.x, y, body.w, &sentence, &F10X20, t.ink);
    y += F10X20.height as i32 + 8;
    if let Some(q) = &m.queue {
        // The queue's view: its band, the newest change in one line, then
        // one page at a time, so nothing runs past its box.
        y = draw_queue(ui, rect(body.x, y, body.w, 64), q, st) + 6;
        y = draw_latest(ui, rect(body.x, y, body.w, 18), q) + 8;
        y = draw_pages(ui, rect(body.x, y, body.w, 24), st) + 8;
        let page = rect(body.x, y, body.w, body.bottom() - y);
        match st.page {
            1 => draw_runs_page(ui, page, q, st),
            2 => draw_found_page(ui, page, q),
            3 => {
                if let Some(i) = draw_net(ui, page, m) {
                    st.zoom = Some(i);
                }
            }
            _ => draw_fleet_rows(ui, page, m),
        }
        return go;
    } else {
        draw_steps(ui, rect(body.x, y, body.w, 34), m, step);
        y += 34 + 10;
    }

    // ---- two columns: the machines, and the net ----------------------------
    let left_w = (body.w * 2 / 5).clamp(300, 460);
    let left = rect(body.x, y, left_w, body.bottom() - y);
    let right = rect(body.x + left_w + 14, y, body.w - left_w - 14, body.bottom() - y);
    draw_machines(ui, left, m);
    if let Some(i) = draw_net(ui, right, m) {
        st.zoom = Some(i);
    }
    go
}

/// The newest change, in one line: the coefficient, its new value and its
/// offset from the default, what the census read, and which machine read
/// it. FOUND when it was an improvement (past two errors, no map below).
/// The rest are on the Found page.
fn draw_latest(ui: &mut Ui, at: Rect, q: &Queue) -> i32 {
    let t = ui.t;
    ui.panel(at, t.tile);
    ui.outline(at, t.line);
    let head = "newest change ";
    let hw = F8X13.measure(head) as i32 + 8;
    ui.label(at.x + 4, at.y + 3, head, &F8X13, t.dim);
    let text = match q.ticker.last() {
        None => "none yet: each finished census reports the coefficients it moved".to_string(),
        Some(k) => format!(
            "{}{} = {}{}{}   read by {} at {}",
            if k.improved { "FOUND IMPROVEMENT  " } else { "" },
            k.var,
            k.value.map(|v| format!("{:.4}", v)).unwrap_or_default(),
            k.offset.map(|o| format!(" ({:+.3} from the default)", o)).unwrap_or_default(),
            match (k.d, k.se) {
                (Some(d), Some(e)) => format!("   census {:+.2} +/- {:.2}", d, e),
                _ => String::new(),
            },
            k.host,
            k.t
        ),
    };
    let found = q.ticker.last().map(|k| k.improved).unwrap_or(false);
    ui.label_in(at.x + hw, at.y + 3, at.w - hw - 6, &text, &F8X13, if found { t.up } else { t.ink });
    at.bottom()
}

/// The page tabs under the band.
fn draw_pages(ui: &mut Ui, at: Rect, st: &mut State) -> i32 {
    let t = ui.t;
    const PAGES: [(&str, &str); 4] = [
        ("Fleet", "each machine and its job"),
        ("Runs", "ask for a run, cancel, approve"),
        ("Found", "improvements, by variable and machine"),
        ("Net", "the learned tank's weights"),
    ];
    let mut x = at.x;
    for (i, (name, what)) in PAGES.iter().enumerate() {
        let w = F8X13.measure(name) as i32 + 24;
        let tab = rect(x, at.y, w, at.h);
        let on = st.page == i;
        ui.panel(tab, if on { t.tile } else { t.panel });
        ui.outline(tab, t.line);
        if on {
            ui.c.rect(tab.x, tab.bottom() - 2, tab.w, 2, t.accent);
        }
        ui.label(tab.x + 12, tab.y + (at.h - F8X13.height as i32) / 2, name, &F8X13, if on { t.ink } else { t.dim });
        if ui.button_hit(tab) {
            st.page = i;
        }
        let _ = what;
        x += w + 4;
    }
    let what = PAGES.get(st.page).map(|p| p.1).unwrap_or("");
    ui.label_in(x + 10, at.y + (at.h - F8X13.height as i32) / 2, at.right() - x - 10, what, &F8X13, t.dim);
    at.bottom()
}

/// The Fleet page: a row a machine, two lines each. CPU is the machine's
/// load against its cores (amber past 90%), GPU the card's use; then what
/// it is running for the queue.
fn draw_fleet_rows(ui: &mut Ui, at: Rect, m: &Model) {
    let t = ui.t;
    let lh = F8X13.height as i32;
    let row_h = lh * 2 + 10;
    let mut y = at.y;
    let qlanes = m.queue.as_ref().map(|q| &q.lanes[..]).unwrap_or(&[]);
    let name_w = F8X13.measure("saskatchewan  ") as i32;
    let cpu_label_w = F8X13.measure("CPU ") as i32;
    let cpu_bar_w = 160;
    let cpu_text_w = F8X13.measure("16.3 of 16 cores busy   ") as i32;
    for mc in &m.machines {
        if y + row_h > at.bottom() {
            break;
        }
        let r = rect(at.x, y, at.w, row_h);
        ui.panel(r, t.tile);
        ui.outline(r, t.line);
        let ql = qlanes.iter().find(|l| l.host == mc.name || (mc.name == "this Mac" && l.host == "mac"));
        let busy = ql.map(|l| l.state == "running" || l.state == "delivering" || l.state == "fetching").unwrap_or(false);
        let (lc, light) = if !mc.up { (t.alarm, Light::Hollow) } else if busy { (t.up, Light::Full) } else { (t.dim, Light::Hollow) };
        let y1 = r.y + 4;
        ui.light(r.x + 10, y1 + lh / 2, 4, light, lc);
        ui.label(r.x + 20, y1, &mc.name, &F8X13, t.ink);
        let mut x = r.x + 20 + name_w;
        if !mc.up {
            ui.label_in(x, y1, r.right() - x - 6, "not answering", &F8X13, t.alarm);
        } else {
            ui.label(x, y1, "CPU", &F8X13, t.dim);
            x += cpu_label_w;
            let frac = if mc.cores > 0 { mc.load / mc.cores as f64 } else { 0.0 };
            bar(ui, rect(x, y1 + 2, cpu_bar_w, 9), frac, if frac > 0.9 { t.warn } else { t.up });
            x += cpu_bar_w + 6;
            ui.label(x, y1, &format!("{:.1} of {} cores busy", mc.load, mc.cores), &F8X13, t.dim);
            x += cpu_text_w;
            if let Some(g) = &mc.gpu {
                ui.label(x, y1, "GPU", &F8X13, t.dim);
                x += cpu_label_w;
                bar(ui, rect(x, y1 + 2, 80, 9), g.busy_pct as f64 / 100.0, t.accent);
                x += 86;
                ui.label(x, y1, &format!("{}% busy", g.busy_pct), &F8X13, t.dim);
                x += F8X13.measure("100% busy   ") as i32;
            }
            if !mc.address.is_empty() && x + F8X13.measure(&mc.address) as i32 + 8 < r.right() {
                ui.label_right(r.right() - 8, y1, &mc.address, &F8X13, t.dim);
            }
        }
        let now = match ql {
            Some(l) if l.state != "idle" && l.state != "stopped" && l.state != "not started" => {
                let when = l.age_s.map(|a| format!(", for {}", ago(a))).unwrap_or_default();
                format!("{} {}: {}{}", capital(&l.state), l.kind, l.title, when)
            }
            Some(l) => format!("Queue lane {}", l.state),
            None => "Not in the queue".to_string(),
        };
        ui.label_in(r.x + 20, y1 + lh + 2, r.w - 28, &now, &F8X13, if busy { t.up } else { t.dim });
        y += row_h + 4;
    }
    if y + lh < at.bottom() {
        ui.label_in(at.x, y + 4, at.w, "CPU: the load average against the machine's cores (amber past 90%).  GPU: the card's use.  Green light: running a job.", &F8X13, t.dim);
    }
}

/// The Runs page: a button a kind of run (operator only), what each does,
/// and every run asked for, with Cancel and Approve.
fn draw_runs_page(ui: &mut Ui, at: Rect, q: &Queue, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 3;
    const KINDS: [(&str, &str, &str); 13] = [
        ("census", "new census trial=@best name=best", "the search's newest best against the defaults, 55 seeds"),
        ("parts", "new parts", "each part of the brain off, against all on"),
        ("switches", "new switches", "each switch that is off, alone"),
        ("roundrobin", "new roundrobin", "every pair of profiles/*.weights"),
        ("spsa", "new spsa", "SPSA on named weights, a step a census"),
        ("tune", "new tune", "the (1 + lambda) evolution strategy"),
        ("train", "new train", "the learned tank, evolution strategies, phase 1"),
        ("gauntlet", "new gauntlet", "single-decision tests of every net"),
        ("clips", "new clips host=mac", "a short GIF of each milestone net (this Mac)"),
        ("school", "new school", "the learned tank's curriculum"),
        ("record", "new record", "recordings for cloning, to calber"),
        ("families", "new families", "every candidate against the families again"),
        ("promote", "new promote candidate=@strongest", "the strongest candidate onto a branch, for approval"),
    ];
    let half = at.w / 2 - 6;
    let mut y = at.y;
    ui.label(at.x, y, if st.operator { "Ask for a run" } else { "Kinds of run (an operator can ask for one)" }, &F8X13, t.ink);
    y += lh;
    let bw = F8X13.measure("roundrobin") as i32 + 16;
    for (label, verb, what) in KINDS.iter() {
        if y + lh > at.bottom() {
            break;
        }
        if st.operator {
            if ui.button(rect(at.x, y - 2, bw, lh), label, true) {
                st.ctl = Some(verb.to_string());
            }
        } else {
            ui.label(at.x + 4, y, label, &F8X13, t.ink);
        }
        ui.label_in(at.x + bw + 8, y, half - bw - 8, what, &F8X13, t.dim);
        y += lh + 2;
    }
    // Every run asked for, newest at the bottom.
    let rx = at.x + half + 12;
    let rw = at.right() - rx;
    let mut y = at.y;
    ui.label(rx, y, "Runs asked for", &F8X13, t.ink);
    y += lh;
    // Approve pushes to the project's branch: say whether this machine can.
    let (pushline, pc) = match &q.push {
        Some((true, at, _)) => (format!("push: ok to {}", at), t.up),
        Some((false, at, why)) => (format!("push: NOT PERMITTED to {} ({}); Approve cannot land", at, why), t.alarm),
        None => ("push: not checked yet (Sync now checks it)".to_string(), t.dim),
    };
    ui.label_in(rx, y, rw, &pushline, &F8X13, pc);
    y += lh + 2;
    if q.runs.is_empty() {
        ui.label_in(rx, y, rw, "none yet", &F8X13, t.dim);
    }
    let shown = ((at.bottom() - y) / (lh + 2)).max(0) as usize;
    let start = q.runs.len().saturating_sub(shown);
    for r in &q.runs[start..] {
        let live = r.status == "running" || r.status == "new";
        let ready = r.kind == "promote" && r.status == "prepared";
        let cw = if st.operator && (live || ready) { 70 } else { 0 };
        let colour = if r.status == "failed" { t.alarm } else if ready { t.up } else if live { t.ink } else { t.dim };
        let line = if r.detail.is_empty() { format!("{} {} {}", r.id, r.kind, r.status) } else { format!("{} {} {}: {}", r.id, r.kind, r.status, r.detail) };
        ui.label_in(rx, y, rw - cw - 6, &line, &F8X13, colour);
        if cw > 0 && live && ui.button(rect(at.right() - cw, y - 2, cw, lh), "Cancel", true) {
            st.ctl = Some(format!("cancel {}", r.id));
        }
        if cw > 0 && ready && ui.button(rect(at.right() - cw, y - 2, cw, lh), "Approve", true) {
            st.ctl = Some(format!("approve {}", r.id));
        }
        y += lh + 2;
    }
}

/// The Found page: Found Improvement by variable and by machine, and the
/// newest changes, one a line.
fn draw_found_page(ui: &mut Ui, at: Rect, q: &Queue) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 3;
    let colw = (at.w - 20) * 28 / 100;
    // By variable.
    let mut y = at.y;
    ui.label(at.x, y, "Found Improvement: variables", &F8X13, t.ink);
    y += lh;
    let top = q.lead_vars.first().map(|v| v.1).unwrap_or(1).max(1);
    let nw = F8X13.measure("longshot_every  ") as i32;
    for (name, n) in &q.lead_vars {
        if y + lh > at.bottom() {
            break;
        }
        ui.label_in(at.x, y, nw, name, &F8X13, t.ink);
        let bw = ((colw - nw - 40) as i64 * n / top) as i32;
        ui.c.rect(at.x + nw, y + 2, bw.max(2), lh - 6, t.up);
        ui.label(at.x + nw + bw + 4, y, &n.to_string(), &F8X13, t.dim);
        y += lh;
    }
    if q.lead_vars.is_empty() {
        ui.label(at.x, y, "none yet", &F8X13, t.dim);
    }
    // By machine.
    let mx = at.x + colw + 10;
    let mut y = at.y;
    ui.label(mx, y, "Found Improvement: machines", &F8X13, t.ink);
    y += lh;
    let top = q.lead_hosts.iter().map(|h| h.1).max().unwrap_or(1).max(1);
    let hw = F8X13.measure("saskatchewan ") as i32;
    let barw = colw - hw - F8X13.measure(" 99 found of 999") as i32 - 6;
    for (name, read, found) in &q.lead_hosts {
        if y + lh > at.bottom() {
            break;
        }
        ui.label_in(mx, y, hw, name, &F8X13, t.ink);
        let b = (barw as i64 * read / top) as i32;
        let f = (barw as i64 * found / top) as i32;
        ui.c.rect(mx + hw, y + 2, b.max(2), lh - 6, t.line);
        ui.c.rect(mx + hw, y + 2, f, lh - 6, t.up);
        ui.label(mx + hw + barw + 6, y, &format!("{} found of {}", found, read), &F8X13, t.dim);
        y += lh;
    }
    if q.lead_hosts.is_empty() {
        ui.label(mx, y, "none yet", &F8X13, t.dim);
    }
    // The newest changes.
    let cx = mx + colw + 10;
    let cw = at.right() - cx;
    let mut y = at.y;
    ui.label(cx, y, "Newest changes (FOUND: an improvement)", &F8X13, t.ink);
    y += lh;
    // Columns: when, found, the variable = value (offset), the machine.
    let host_w = F8X13.measure("saskatchewan") as i32;
    let when_w = F8X13.measure("00:00:00 FOUND ") as i32;
    for k in q.ticker.iter().rev() {
        if y + lh > at.bottom() {
            break;
        }
        let c = if k.improved { t.up } else { t.dim };
        ui.label(cx, y, &format!("{}{}", k.t, if k.improved { " FOUND" } else { "" }), &F8X13, c);
        let what = format!(
            "{} = {}{}",
            k.var,
            k.value.map(|v| format!("{:.3}", v)).unwrap_or_default(),
            k.offset.map(|o| format!(" ({:+.2})", o)).unwrap_or_default()
        );
        ui.label_in(cx + when_w, y, cw - when_w - host_w - 8, &what, &F8X13, c);
        ui.label_right(at.right(), y, &k.host, &F8X13, c);
        y += lh;
    }
}

/// The sentence on top, and which step (0 record, 1 train, 2 judge) is lit.
fn headline(m: &Model) -> (String, Option<usize>) {
    let project = if m.project.is_empty() { "the net".to_string() } else { format!("{}'s learned tank", m.project) };
    if let Some(q) = &m.queue {
        let sum = |i: usize| q.counts.iter().map(|(_, c)| c[i]).sum::<i64>();
        let what = match q.state.as_str() {
            "running, adding" => "keeping the fleet busy",
            "draining: adding nothing" => "draining: finishing what is queued, adding nothing",
            "stopping now" => "stopping now",
            "shut down" => "shut down",
            s if s.starts_with("shutting down") => "shutting down: each machine finishes, reports, powers off",
            _ => "stopped",
        };
        return (
            format!("Queue {}: {}. {} queued, {} running, {} done.", q.name, what, sum(0), sum(1), sum(2)),
            None,
        );
    }
    if m.looping && m.round > 0 {
        let at = m.stages.iter().position(|(_, st)| st == "started");
        let what = match at {
            Some(0) => "recording new matches on the fleet",
            Some(1) => "training the net on calber's GPU",
            Some(2) => "judging the new net in the census",
            _ => "between steps",
        };
        return (format!("Teaching {}: round {}, {}.", project, m.round, what), at);
    }
    match &m.run {
        Some(r) if !r.ended => (
            format!("Recording matches to teach {}: {} of {} jobs, {} matches so far.", project, r.done, r.total, r.seeds),
            Some(0),
        ),
        Some(r) => (format!("Idle. The last run, {}, recorded {} matches for training.", r.name, r.seeds), None),
        None => ("Idle: nothing is recording or training.".into(), None),
    }
}

fn draw_steps(ui: &mut Ui, at: Rect, m: &Model, lit: Option<usize>) {
    let t = ui.t;
    let recorders = m.run.as_ref().map(|r| r.lanes.len()).unwrap_or(0);
    let steps = [
        ("1  Record", format!("the scripted brain plays; {} machines watch", recorders)),
        ("2  Train", "calber's GPU learns to copy it".to_string()),
        ("3  Judge", "a census on columbia: better than the best?".to_string()),
    ];
    let gap = 26;
    let bw = (at.w - gap * 2) / 3;
    for (i, (name, what)) in steps.iter().enumerate() {
        let b = rect(at.x + i as i32 * (bw + gap), at.y, bw, at.h);
        let on = lit == Some(i);
        ui.panel(b, if on { mix(t.tile, t.up, 70) } else { t.tile });
        ui.outline(b, if on { t.up } else { t.line });
        ui.label(b.x + 8, b.y + 3, name, &F8X13, if on { t.ink } else { t.dim });
        ui.label_in(b.x + 8, b.y + 3 + F8X13.height as i32 + 2, b.w - 12, what, &F8X13, t.dim);
        if i < 2 {
            let ax = b.right() + 6;
            let ay = b.y + b.h / 2;
            ui.c.hline(ax, ay, gap - 12, t.dim);
            ui.c.rect(ax + gap - 15, ay - 2, 3, 5, t.dim);
        }
    }
}

/// The queue band: one bar of done, running and queued; the counts of each
/// kind; what comes next; and, for an operator, the three controls.
fn draw_queue(ui: &mut Ui, at: Rect, q: &Queue, st: &mut State) -> i32 {
    let t = ui.t;
    let sum = |i: usize| q.counts.iter().map(|(_, c)| c[i]).sum::<i64>();
    let (queued, running, done, failed) = (sum(0), sum(1), sum(2), sum(3) + sum(4));
    let total = (queued + running + done + failed).max(1);
    let bw = if st.operator { at.w - 430 } else { at.w };
    let bar = rect(at.x, at.y, bw, 12);
    ui.panel(bar, t.tile);
    ui.outline(bar, t.line);
    let mut x = bar.x + 1;
    for (n, c) in [(done, t.up), (running, t.accent), (failed, t.alarm)] {
        let w = ((bar.w - 2) as i64 * n / total) as i32;
        ui.c.rect(x, bar.y + 1, w, bar.h - 2, c);
        x += w;
    }
    let mut ly = at.y + 16;
    // The bar's key: it counts the queue's jobs, not a machine's load.
    // (label returns the x where its text ends.)
    let mut kx = ui.label(at.x, ly, "jobs: ", &F8X13, t.dim) + 2;
    for (label, n, c) in [("done", done, t.up), ("running", running, t.accent), ("queued", queued, t.line), ("failed or stopped", failed, t.alarm)] {
        if n == 0 && label.starts_with("failed") {
            continue;
        }
        ui.c.rect(kx, ly + 3, 9, 9, c);
        kx = ui.label(kx + 13, ly, &format!("{} {}", n, label), &F8X13, t.dim) + 18;
    }
    ly += F8X13.height as i32 + 3;
    if let Some((k, title, ok)) = q.recent.last() {
        ui.label_in(at.x, ly, bw, &format!("last finished: {} - {}{}", k, title, if *ok { "" } else { "  (failed or stopped)" }), &F8X13, if *ok { t.dim } else { t.alarm });
        ly += F8X13.height as i32 + 3;
    }
    if !q.synced.is_empty() {
        ui.label_in(at.x, ly, bw, &q.synced, &F8X13, t.dim);
        ly += F8X13.height as i32 + 3;
    }
    if !q.shutdown.is_empty() {
        ui.label_in(at.x, ly, bw, &format!("shutdown: {}", q.shutdown), &F8X13, t.ink);
        ly += F8X13.height as i32 + 3;
    }
    if st.operator {
        // Absent rather than greyed when they cannot apply: a running queue
        // offers stop adding; a draining one, resume; both offer stop now.
        let bx = at.right() - 420;
        let running_keeper = q.state != "stopped";
        let mut x = bx;
        if running_keeper && q.adding && ui.button(rect(x, at.y, 130, 26), "Stop adding", true) {
            st.ctl = Some("stop-adding".into());
        }
        if running_keeper && !q.adding && ui.button(rect(x, at.y, 130, 26), "Resume adding", true) {
            st.ctl = Some("resume-adding".into());
        }
        if !running_keeper && ui.button(rect(x, at.y, 130, 26), "Start the queue", true) {
            st.ctl = Some("ensure".into());
        }
        x += 140;
        if running_keeper && ui.button(rect(x, at.y, 280, 26), "Collect results and stop now", true) {
            st.ctl = Some("stop-now".into());
        }
        // The second row: gather now, and shut the fleet down. Shutdown
        // powers machines off, so it asks twice: the first press arms it.
        let y2 = at.y + 32;
        if ui.button(rect(bx, y2, 130, 26), "Sync now", true) {
            st.ctl = Some("sync".into());
        }
        let shutting = q.state.starts_with("shutting down") || q.state == "shut down";
        if running_keeper && !shutting {
            let label = if st.arm_shutdown { "Press again: shut the fleet down" } else { "Shut the fleet down" };
            if ui.button(rect(bx + 140, y2, 280, 26), label, true) {
                if st.arm_shutdown {
                    st.ctl = Some("shutdown-now".into());
                    st.arm_shutdown = false;
                } else {
                    st.arm_shutdown = true;
                }
            }
        }
        if !st.said.is_empty() {
            ui.label_in(bx, at.y + 64, 420, &st.said, &F8X13, t.dim);
        }
    }
    ly.max(at.y + if st.operator { 82 } else { 48 })
}

fn draw_machines(ui: &mut Ui, at: Rect, m: &Model) {
    let t = ui.t;
    ui.label(at.x, at.y, "The fleet", &F8X13, t.ink);
    ui.label(at.x + F8X13.measure("The fleet ") as i32, at.y, "- each machine, and what it is doing", &F8X13, t.dim);
    let mut y = at.y + F8X13.height as i32 + 6;
    let lanes = m.run.as_ref().map(|r| (&r.lanes[..], r.ended)).unwrap_or((&[], true));
    let n = m.machines.len().max(1) as i32;
    let card_h = ((at.bottom() - y) / n - 6).clamp(64, 96);
    for mc in &m.machines {
        if y + card_h > at.bottom() + 2 {
            break;
        }
        let c = rect(at.x, y, at.w, card_h);
        ui.panel(c, t.tile);
        ui.outline(c, t.line);
        let lane = lanes.0.iter().find(|l| l.host == mc.name || (mc.name == "this Mac" && l.host == "mac"));
        let qlane = m.queue.as_ref().and_then(|q| q.lanes.iter().find(|l| l.host == mc.name || (mc.name == "this Mac" && l.host == "mac")));
        let busy = lane.map(|l| l.state == "recording" || l.state == "sending").unwrap_or(false)
            || qlane.map(|l| l.state == "running" || l.state == "delivering" || l.state == "fetching").unwrap_or(false);
        let (lc, light) = if !mc.up {
            (t.alarm, Light::Hollow)
        } else if busy {
            (t.up, Light::Full)
        } else {
            (t.dim, Light::Hollow)
        };
        let mut ly = c.y + 5;
        ui.light(c.x + 10, ly + 9, 4, light, lc);
        let nx = c.x + 20 + ui.label(c.x + 20, ly, &mc.name, &F10X20, t.ink);
        let what = if mc.roles.is_empty() { "records matches".to_string() } else { mc.roles.join(", ") };
        let tag = if mc.address.is_empty() { what } else { format!("{}  {}", mc.address, what) };
        ui.label_in(nx + 10, ly + 5, c.right() - nx - 16, &tag, &F8X13, t.dim);
        ly += F10X20.height as i32 + 3;
        if !mc.up {
            ui.label_in(c.x + 20, ly, c.w - 28, "not answering", &F8X13, t.alarm);
            y += card_h + 6;
            continue;
        }
        // Busy: the load against the cores, and a GPU's own use beside it.
        let lab = format!("CPU {:.1} of {}", mc.load, mc.cores);
        ui.label(c.x + 20, ly, &lab, &F8X13, t.dim);
        let bx = c.x + 20 + F8X13.measure("CPU 00.0 of 24 ") as i32;
        let frac = if mc.cores > 0 { mc.load / mc.cores as f64 } else { 0.0 };
        let bw = if mc.gpu.is_some() { (c.right() - bx - 12) / 2 - 8 } else { c.right() - bx - 12 };
        bar(ui, rect(bx, ly + 2, bw, 9), frac, if frac > 0.9 { t.warn } else { t.up });
        if let Some(g) = &mc.gpu {
            let gx = bx + bw + 16;
            let gl = format!("GPU {}%", g.busy_pct);
            ui.label(gx, ly, &gl, &F8X13, t.dim);
            let gbx = gx + F8X13.measure("GPU 100% ") as i32;
            bar(ui, rect(gbx, ly + 2, c.right() - gbx - 12, 9), g.busy_pct as f64 / 100.0, t.accent);
        }
        ly += F8X13.height as i32 + 4;
        let now = match (qlane, lane) {
            (Some(ql), _) if ql.state != "idle" && ql.state != "stopped" && ql.state != "not started" => {
                let when = ql.age_s.map(|a| format!(", for {}", ago(a))).unwrap_or_default();
                format!("{} {}: {}{}", capital(&ql.state), ql.kind, ql.title, when)
            }
            (Some(ql), _) => format!("Queue lane {}", ql.state),
            (None, lane) => match lane {
            Some(l) if !lanes.1 || l.state != "finished" => {
                doing(l, m.run.as_ref().map(|r| !r.store.is_empty() && r.store == mc.address).unwrap_or(false))
            }
            Some(l) => format!("Done: {} jobs in the last run", l.jobs),
            None if mc.name == "saskatchewan" && frac > 0.9 => "Busy with its own work (make converge)".into(),
            None => "Not recording in this run".into(),
            },
        };
        ui.label_in(c.x + 20, ly, c.w - 28, &now, &F8X13, if busy { t.up } else { t.dim });
        ly += F8X13.height as i32 + 2;
        if let Some(l) = lane {
            if busy && !l.pipe.is_empty() && ly + F8X13.height as i32 <= c.bottom() - 2 {
                ui.label_in(c.x + 20, ly, c.w - 28, &format!("$ {}", l.pipe), &F8X13, t.dim);
            }
        }
        y += card_h + 6;
    }
}

/// The colour of weight `v` in a layer whose largest weight is `max`: the
/// sign picks the hue (up positive, alarm negative), the size its strength,
/// on a square-root scale so that small weights still show.
fn weight_colour(ui: &Ui, v: f64, max: f64) -> Rgb {
    let t = ui.t;
    let k = (v.abs() / max.max(1e-9)).sqrt().min(1.0);
    mix(t.tile, if v >= 0.0 { t.up } else { t.alarm }, (k * 255.0) as u8)
}

/// A colour bar from -max through 0 to +max, `r` wide, labelled under it.
fn legend(ui: &mut Ui, r: Rect, max: f64, vertical: bool) {
    let t = ui.t;
    let steps = if vertical { r.h } else { r.w };
    for k in 0..steps {
        // -1 .. 1 along the bar; the value is that squared, signed, times
        // max: the inverse of weight_colour's scale.
        let p = 2.0 * k as f64 / (steps - 1).max(1) as f64 - 1.0;
        let p = if vertical { -p } else { p };
        let c = weight_colour(ui, p.signum() * p * p * max, max);
        if vertical {
            ui.c.rect(r.x, r.y + k, r.w, 1, c);
        } else {
            ui.c.rect(r.x + k, r.y, 1, r.h, c);
        }
    }
    ui.outline(r, t.line);
    let marks = [(-1.0, format!("{:+.2}", -max)), (-0.5, format!("{:+.2}", -max / 4.0)), (0.0, "0".to_string()), (0.5, format!("{:+.2}", max / 4.0)), (1.0, format!("{:+.2}", max))];
    for (p, label) in marks.iter() {
        if vertical {
            let y = r.y + (((1.0 - p) / 2.0) * (r.h - 1) as f64) as i32;
            ui.c.rect(r.right(), y, 4, 1, t.dim);
            ui.label(r.right() + 7, y - F8X13.height as i32 / 2, label, &F8X13, t.dim);
        } else {
            let x = r.x + (((p + 1.0) / 2.0) * (r.w - 1) as f64) as i32;
            ui.c.rect(x, r.bottom(), 1, 4, t.dim);
            let lw = F8X13.measure(label) as i32;
            let lx = (x - lw / 2).clamp(r.x - 20, r.right() + 20 - lw);
            ui.label(lx, r.bottom() + 5, label, &F8X13, t.dim);
        }
    }
}

const LAYER_NAMES: [&str; 3] = ["senses -> layer 1", "layer 1 -> layer 2", "layer 2 -> scores"];

/// The groups along one side of layer `i`'s matrix: the senses' groups for
/// the first layer's inputs, the heads for the last layer's outputs, and the
/// hidden units otherwise (as a group per 16).
fn axis_groups(net: &Net, i: usize, inputs: bool) -> Vec<(String, i64, i64)> {
    let last = net.layers.len().saturating_sub(1);
    if inputs && i == 0 && !net.input_groups.is_empty() {
        return net.input_groups.clone();
    }
    if !inputs && i == last && !net.output_groups.is_empty() {
        return net.output_groups.clone();
    }
    let n = net.layers.get(i).map(|l| if inputs { l.inputs } else { l.outputs }).unwrap_or(0);
    (0..n).step_by(16).map(|a| (format!("{}-{}", a, (a + 15).min(n - 1)), a, (a + 15).min(n - 1))).collect()
}

/// One matrix, large: its axes labelled with what the inputs and outputs
/// are, a legend, and the weight under the pointer. Returns true to close.
fn draw_zoom(ui: &mut Ui, at: Rect, net: &Net, i: usize) -> bool {
    let t = ui.t;
    let l = &net.layers[i];
    ui.panel(at, t.base);
    let title = format!(
        "{}: {} inputs across, {} outputs down; weights from {:+.2} to {:+.2}",
        LAYER_NAMES.get(i).copied().unwrap_or("layer"),
        l.inputs,
        l.outputs,
        -l.max_abs,
        l.max_abs
    );
    ui.label_in(at.x, at.y, at.w - 110, &title, &F10X20, t.ink);
    let close = rect(at.right() - 90, at.y, 90, 24);
    let closed = ui.button(close, "Close", true);
    let hint_y = at.y + F10X20.height as i32 + 6;

    let rows = l.cells.len().max(1) as i32;
    let cols = l.cells.first().map(|r| r.len()).unwrap_or(1).max(1) as i32;
    let xg = axis_groups(net, i, true);
    let yg = axis_groups(net, i, false);
    let ml = yg.iter().map(|g| F8X13.measure(&g.0) as i32).max().unwrap_or(20) + 28;
    let (mt, mb, mr) = (hint_y + F8X13.height as i32 + 10 - at.y, 52, 110);
    let cell = ((at.w - ml - mr) / cols).min((at.h - mt - mb) / rows).max(1);
    let (gw, gh) = (cell * cols, cell * rows);
    let (gx, gy) = (at.x + ml, at.y + mt);

    for (r, row) in l.cells.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            ui.c.rect(gx + c as i32 * cell, gy + r as i32 * cell, cell, cell, weight_colour(ui, *v, l.max_abs));
        }
    }
    ui.outline(rect(gx - 1, gy - 1, gw + 2, gh + 2), t.line);

    // The x axis: a bracket a group, its name under it (shortened to fit).
    for (k, (name, a, b)) in xg.iter().enumerate() {
        let x0 = gx + *a as i32 * cell;
        let x1 = gx + (*b as i32 + 1) * cell;
        ui.c.rect(x0, gy + gh + 2, 1, 6, t.dim);
        ui.c.hline(x0, gy + gh + 7, x1 - x0, t.dim);
        let room = x1 - x0 - 2;
        let lab = if F8X13.measure(name) as i32 <= room { name.clone() } else { name.chars().take(((room / 8).max(1)) as usize).collect() };
        let ly = gy + gh + 10 + if k % 2 == 1 && room < 40 { F8X13.height as i32 } else { 0 };
        ui.label(x0 + (room - F8X13.measure(&lab) as i32) / 2 + 1, ly, &lab, &F8X13, t.ink);
    }
    let xt = if i == 0 { "inputs: the 143 senses, by group" } else { "inputs: the units of the layer before" };
    ui.label(gx, gy + gh + 38, xt, &F8X13, t.dim);

    // The y axis: the same, down the left.
    for (name, a, b) in yg.iter() {
        let y0 = gy + *a as i32 * cell;
        let y1 = gy + (*b as i32 + 1) * cell;
        ui.c.rect(gx - 8, y0, 6, 1, t.dim);
        ui.c.vline(gx - 7, y0, y1 - y0, t.dim);
        let ty = y0 + (y1 - y0 - F8X13.height as i32) / 2;
        if y1 - y0 >= F8X13.height as i32 - 2 {
            ui.label_right(gx - 12, ty, name, &F8X13, t.ink);
        }
    }
    let last = net.layers.len().saturating_sub(1);
    let yt = if i == last { "outputs: the 47 scores, by head" } else { "outputs: this layer's 64 units" };
    ui.label(at.x, hint_y, yt, &F8X13, t.dim);

    // The legend, upright beside the matrix.
    let lr = rect(gx + gw + 24, gy, 14, gh.min(260).max(80));
    legend(ui, lr, l.max_abs, true);
    ui.label(lr.x - 4, lr.bottom() + 8, "weight", &F8X13, t.dim);

    // The weight under the pointer.
    let (px, py) = ui.pointer();
    let readout = if px >= gx && py >= gy && px < gx + gw && py < gy + gh {
        let (c, r) = ((px - gx) / cell, (py - gy) / cell);
        let v = l.cells.get(r as usize).and_then(|row| row.get(c as usize)).copied().unwrap_or(0.0);
        ui.outline(rect(gx + c * cell - 1, gy + r * cell - 1, cell + 2, cell + 2), t.ink);
        let gname = |g: &[(String, i64, i64)], k: i64| g.iter().find(|x| k >= x.1 && k <= x.2).map(|x| format!(" ({})", x.0)).unwrap_or_default();
        format!("output {}{}, input {}{}: weight {:+.3}", r, gname(&yg, r as i64), c, gname(&xg, c as i64), v)
    } else {
        "point at a cell to read its weight; Escape or Close to go back".to_string()
    };
    ui.label_in(at.x + 260, hint_y, at.w - 260, &readout, &F8X13, t.ink);
    closed
}

/// The net, small. Returns the matrix clicked, to enlarge.
fn draw_net(ui: &mut Ui, at: Rect, m: &Model) -> Option<usize> {
    let t = ui.t;
    let Some(net) = &m.net else {
        ui.label_in(at.x, at.y, at.w, "No net to show yet.", &F8X13, t.dim);
        return None;
    };
    let mut clicked = None;
    let title = format!("The net being trained: {}", net.name);
    ui.label_in(at.x, at.y, at.w, &title, &F8X13, t.ink);
    let mut y = at.y + F8X13.height as i32 + 4;
    let sizes: Vec<String> = net.sizes.iter().map(|s| s.to_string()).collect();
    let line = format!("{} numbers: {} senses in, two layers of {}, {} scores out", net.parameters, sizes[0], sizes.get(1).cloned().unwrap_or_default(), sizes.last().cloned().unwrap_or_default());
    ui.label_in(at.x, y, at.w, &line, &F8X13, t.dim);
    y += F8X13.height as i32 + 8;

    // The three weight matrices, as heat maps: a cell a group of weights,
    // its colour the sign (up for positive, alarm for negative), its strength
    // the size. Inputs run across, outputs down.
    let labels = LAYER_NAMES;
    let gap = 14;
    let side = ((at.w - gap * 2) / 3).min(150).max(60);
    let mut x = at.x;
    let mut hover: Option<String> = None;
    for (i, l) in net.layers.iter().enumerate() {
        let rows = l.cells.len().max(1) as i32;
        let cols = l.cells.first().map(|r| r.len()).unwrap_or(1).max(1) as i32;
        // Each cell a weight when there is room for it, else the matrix drawn
        // smaller than one pixel a weight: a box of `side`, sampled.
        let (gw, gh) = if cols >= rows { (side, (side * rows / cols).max(8)) } else { ((side * cols / rows).max(8), side) };
        ui.label_in(x, y, side + 10, labels.get(i).copied().unwrap_or(""), &F8X13, t.dim);
        let gy = y + F8X13.height as i32 + 3;
        for py in 0..gh {
            let r = (py * rows / gh) as usize;
            for px in 0..gw {
                let c = (px * cols / gw) as usize;
                if let Some(v) = l.cells.get(r).and_then(|row| row.get(c)) {
                    ui.c.rect(x + px, gy + py, 1, 1, weight_colour(ui, *v, l.max_abs));
                }
            }
        }
        let boxr = rect(x - 1, gy - 1, gw + 2, gh + 2);
        let hot = ui.is_hot(boxr);
        ui.outline(boxr, if hot { t.ink } else { t.line });
        // The cell under the pointer: what its row and column are, and its
        // weight, read without enlarging.
        let (px, py) = ui.pointer();
        if px >= x && py >= gy && px < x + gw && py < gy + gh {
            let (r, c) = (((py - gy) * rows / gh) as usize, ((px - x) * cols / gw) as usize);
            if let Some(v) = l.cells.get(r).and_then(|row| row.get(c)) {
                let xg = axis_groups(net, i, true);
                let yg = axis_groups(net, i, false);
                let gname = |g: &[(String, i64, i64)], k: i64| g.iter().find(|x| k >= x.1 && k <= x.2).map(|x| format!(" {}", x.0)).unwrap_or_default();
                hover = Some(format!("{}: output {}{}, input {}{}: weight {:+.3}", labels.get(i).copied().unwrap_or(""), r, gname(&yg, r as i64), c, gname(&xg, c as i64), v));
                ui.c.rect(px - 3, py, 7, 1, t.ink);
                ui.c.rect(px, py - 3, 1, 7, t.ink);
            }
        }
        if ui.button_hit(boxr) {
            clicked = Some(i);
        }
        let dims = format!("{} x {}  {}", l.inputs, l.outputs, if hot { "click to enlarge" } else { "" });
        ui.label_in(x, gy + gh + 3, side + gap, &dims, &F8X13, if hot { t.ink } else { t.dim });
        x += side + gap;
    }
    let tallest = net.layers.iter().map(|l| {
        let rows = l.cells.len().max(1) as i32;
        let cols = l.cells.first().map(|r| r.len()).unwrap_or(1).max(1) as i32;
        if cols >= rows { (side * rows / cols).max(8) } else { side }
    }).max().unwrap_or(side);
    y += F8X13.height as i32 + 3 + tallest + F8X13.height as i32 + 8;
    // The legend for all three: the colour of a weight.
    let lw = (at.w / 3).clamp(120, 220);
    let max = net.layers.iter().map(|l| l.max_abs).fold(0.0, f64::max);
    ui.label(at.x, y, "weight:", &F8X13, t.dim);
    legend(ui, rect(at.x + 64, y + 2, lw, 9), max, false);
    let hint = hover.unwrap_or_else(|| "red takes away, green adds; point at a cell to read it, click to enlarge".to_string());
    ui.label_in(at.x + 64 + lw + 26, y, at.right() - (at.x + 64 + lw + 26), &hint, &F8X13, t.dim);
    y += 9 + F8X13.height as i32 + 16;

    // The 47 scores, split into the heads they belong to.
    let total: i64 = net.heads.iter().map(|(_, c)| *c).sum::<i64>().max(1);
    let mut hx = at.x;
    let hb = at.w;
    for (i, (name, c)) in net.heads.iter().enumerate() {
        let hw = (hb as i64 * c / total) as i32;
        let r = rect(hx, y, hw - 2, 18);
        ui.panel(r, if i % 2 == 0 { mix(t.tile, t.accent, 60) } else { mix(t.tile, t.accent, 30) });
        ui.label_in(r.x + 3, r.y + 3, r.w - 4, &format!("{} {}", name, c), &F8X13, t.ink);
        hx += hw;
    }
    y += 18 + 4;
    ui.label_in(at.x, y, at.w, "the 47 scores, by what they choose: where to drive, where to shoot, when to fire the mortar ...", &F8X13, t.dim);
    y += F8X13.height as i32 + 12;

    // How often it chooses what the scripted brain chose.
    if let Some(q) = &m.quality {
        ui.label_in(at.x, y, at.w, "Makes the scripted brain's choice (unseen matches)", &F8X13, t.ink);
        y += F8X13.height as i32 + 4;
        let name_w = F8X13.measure("mortar ") as i32;
        let bx = at.x + name_w;
        let bw = at.w - name_w - F8X13.measure(" 100.0%") as i32 - 4;
        for (head, now, bc1, base) in &q.heads {
            if y + 14 > at.bottom() - 34 {
                break;
            }
            ui.label(at.x, y, head, &F8X13, t.dim);
            let r = rect(bx, y + 1, bw, 11);
            ui.panel(r, t.base);
            ui.outline(r, t.line);
            if now.is_finite() {
                ui.c.rect(r.x + 1, r.y + 1, ((r.w - 2) as f64 * now / 100.0) as i32, r.h - 2, t.up);
            }
            // bc1 and always-the-commonest-choice as ticks on the same bar.
            for (v, c) in [(*bc1, t.ink), (*base, t.alarm)] {
                if v.is_finite() {
                    let tx = r.x + 1 + ((r.w - 2) as f64 * v / 100.0) as i32;
                    ui.c.rect(tx - 1, r.y - 2, 2, r.h + 4, c);
                }
            }
            ui.label(r.right() + 4, y, &format!("{:.1}%", now), &F8X13, t.ink);
            y += 15;
        }
        ui.label_in(at.x, y, at.w, "bar: this net   dark tick: the first clone (bc1)   red tick: always the commonest choice", &F8X13, t.dim);
        y += F8X13.height as i32 + 8;
        if let Some((best, bse, off, ose, vs, vse)) = q.census {
            if y + 2 * F8X13.height as i32 <= at.bottom() {
                let verdict = if vs > 2.0 * vse {
                    "better than the official net: Paul decides"
                } else if vs < -2.0 * vse {
                    "worse than the official net"
                } else {
                    "level with the official net"
                };
                ui.label_in(at.x, y, at.w, &format!("In the census: {:+.2} a seat against the scripted brain (+/- {:.2})", best, bse), &F8X13, t.ink);
                y += F8X13.height as i32 + 2;
                ui.label_in(at.x, y, at.w, &format!("official map7: {:+.2} (+/- {:.2}); this net is {:+.2} +/- {:.2}: {}", off, ose, vs, vse, verdict), &F8X13, t.dim);
            }
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"fleet":"grove","project":"treadcore",
      "machines":[{"name":"calber","address":"192.168.8.181","roles":["trains on the GPU"],"up":true,"cores":8,"load":4.1,
                   "gpu":{"name":"GTX 1080 Ti","busy_pct":0,"mem_mb":0,"mem_total_mb":11264}},
                  {"name":"this Mac","address":"","roles":["coordinates"],"up":true,"cores":8,"load":3.9}],
      "run":{"run":"rec2400","done":137,"total":200,"running":4,"seeds":1644,"ended":false,
             "lanes":[{"host":"mac","jobs":50,"state":"recording","job":"9002800","age_s":12,"pipe":"RECORD=..."}]},
      "loop":{"round":0,"stages":[{"stage":"record","state":""}],"best":"","running":false},
      "net":{"name":"bc2","sizes":[143,64,64,47],"parameters":16431,"heads":[{"name":"drive","choices":10}],
             "layers":[{"inputs":143,"outputs":64,"max_abs":1.0,"cells":[[0.5,-0.5],[0.1,0.0]]}]},
      "quality":{"net":"bc2","held_out":{"drive":66.6},"bc1":{"drive":65.5},"commonest":{"drive":36.5},
                 "census":{"best":-19.76,"best_se":0.39,"official":-20.33,"official_se":0.27,"vs_official":0.57,"vs_official_se":0.37}}}"#;

    #[test]
    fn the_document_reads_whole() {
        let m = Model::parse(DOC, "test");
        assert!(m.why.is_empty(), "{}", m.why);
        assert_eq!(m.machines.len(), 2);
        assert_eq!(m.machines[0].gpu.as_ref().unwrap().mem_total_mb, 11264);
        assert!(m.machines[1].gpu.is_none());
        assert_eq!(m.run.as_ref().unwrap().lanes[0].host, "mac");
        assert_eq!(m.net.as_ref().unwrap().layers[0].cells[0], vec![0.5, -0.5]);
        assert_eq!(m.quality.as_ref().unwrap().heads[0].1, 66.6);
        assert!(m.quality.as_ref().unwrap().heads[1].1.is_nan(), "a head the document lacks is not a zero");
    }

    #[test]
    fn the_sentence_says_what_is_happening() {
        let m = Model::parse(DOC, "test");
        let (s, step) = headline(&m);
        assert!(s.contains("137 of 200 jobs"), "{}", s);
        assert!(s.len() <= 90, "the sentence fits a 960-wide window: {}", s);
        assert_eq!(step, Some(0));
    }

    #[test]
    fn a_lane_says_what_it_does_in_words() {
        let l = Lane { state: "sending".into(), job: "9001716".into(), age_s: Some(4), ..Lane::default() };
        assert_eq!(doing(&l, false), "Sending matches 9001716 to the GPU machine, for 4 s");
        assert_eq!(doing(&l, true), "Filing matches 9001716 and checking them, for 4 s");
    }

    #[test]
    fn no_command_and_bad_output_each_say_why() {
        assert!(Model::parse("", "").why.contains("--training-cmd"));
        assert!(Model::parse("nope", "x").why.contains("did not print JSON"));
    }
}
