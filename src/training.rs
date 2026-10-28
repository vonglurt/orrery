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
//! command prints (`--training-cmd`; treadcore's is `scripts/fleet-view.py`),
//! polled on a thread. Nothing is discovered here, and there are no verbs.

use crate::draw::{mix, Rgb};
use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
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
                "Nothing to show yet: start orrery with --training-cmd, e.g. \
                 \"cd ~/code/treadcore && python3 -I scripts/fleet-view.py\""
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
        if let Some(x) = v.get("net").filter(|x| x.get("sizes").is_some()) {
            m.net = Some(Net {
                name: s(x, "name"),
                sizes: arr(x, "sizes").iter().filter_map(|v| v.as_num()).map(|v| v as i64).collect(),
                parameters: n(x, "parameters") as i64,
                heads: arr(x, "heads").iter().map(|h| (s(h, "name"), n(h, "choices") as i64)).collect(),
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

/// A horizontal bar: `frac` of it filled.
fn bar(ui: &mut Ui, r: Rect, frac: f64, c: Rgb) {
    let t = ui.t;
    ui.panel(r, t.base);
    ui.outline(r, t.line);
    let w = ((r.w - 2) as f64 * frac.clamp(0.0, 1.0)).round() as i32;
    ui.c.rect(r.x + 1, r.y + 1, w, r.h - 2, c);
}

pub fn draw(ui: &mut Ui, m: &Model) -> Option<nav::View> {
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
    let (sentence, step) = headline(m);
    ui.label_in(body.x, y, body.w, &sentence, &F10X20, t.ink);
    y += F10X20.height as i32 + 8;
    draw_steps(ui, rect(body.x, y, body.w, 34), m, step);
    y += 34 + 10;

    // ---- two columns: the machines, and the net ----------------------------
    let left_w = (body.w * 2 / 5).clamp(300, 460);
    let left = rect(body.x, y, left_w, body.bottom() - y);
    let right = rect(body.x + left_w + 14, y, body.w - left_w - 14, body.bottom() - y);
    draw_machines(ui, left, m);
    draw_net(ui, right, m);
    go
}

/// The sentence on top, and which step (0 record, 1 train, 2 judge) is lit.
fn headline(m: &Model) -> (String, Option<usize>) {
    let project = if m.project.is_empty() { "the net".to_string() } else { format!("{}'s learned tank", m.project) };
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
        let busy = lane.map(|l| l.state == "recording" || l.state == "sending").unwrap_or(false);
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
        let now = match lane {
            Some(l) if !lanes.1 || l.state != "finished" => {
                doing(l, m.run.as_ref().map(|r| !r.store.is_empty() && r.store == mc.address).unwrap_or(false))
            }
            Some(l) => format!("Done: {} jobs in the last run", l.jobs),
            None if mc.name == "saskatchewan" && frac > 0.9 => "Busy with its own work (make converge)".into(),
            None => "Not recording in this run".into(),
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

fn draw_net(ui: &mut Ui, at: Rect, m: &Model) {
    let t = ui.t;
    let Some(net) = &m.net else {
        ui.label_in(at.x, at.y, at.w, "No net to show yet.", &F8X13, t.dim);
        return;
    };
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
    let labels = ["senses -> layer 1", "layer 1 -> layer 2", "layer 2 -> scores"];
    let gap = 14;
    let side = ((at.w - gap * 2) / 3).min(150).max(60);
    let mut x = at.x;
    for (i, l) in net.layers.iter().enumerate() {
        let rows = l.cells.len().max(1) as i32;
        let cols = l.cells.first().map(|r| r.len()).unwrap_or(1).max(1) as i32;
        let cell = (side / cols.max(rows)).max(1);
        let (gw, gh) = (cell * cols, cell * rows);
        ui.label_in(x, y, side + 10, labels.get(i).copied().unwrap_or(""), &F8X13, t.dim);
        let gy = y + F8X13.height as i32 + 3;
        for (r, row) in l.cells.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let k = (v.abs() / l.max_abs.max(1e-9)).sqrt().min(1.0);
                let hue = if *v >= 0.0 { t.up } else { t.alarm };
                ui.c.rect(x + c as i32 * cell, gy + r as i32 * cell, cell, cell, mix(t.tile, hue, (k * 255.0) as u8));
            }
        }
        ui.outline(rect(x - 1, gy - 1, gw + 2, gh + 2), t.line);
        let dims = format!("{} x {}", l.inputs, l.outputs);
        ui.label(x, gy + gh + 3, &dims, &F8X13, t.dim);
        x += side + gap;
    }
    y += F8X13.height as i32 + 3 + side + F8X13.height as i32 + 10;

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
