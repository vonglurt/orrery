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
    /// Do it now was pressed once: the next press does it.
    pub arm_now: bool,
    /// The Candidates page: the kind shown (0 all, 1 promoted, 2 current,
    /// 3 older) and the first row.
    pub cand_filter: u8,
    pub cand_off: usize,
    /// Collect results and stop now, armed from the menu: the button says
    /// so, and its press runs it.
    pub arm_stop: bool,
    /// Which page of the queue's view: 0 Fleet, 1 Runs, 2 Found, 3 Net.
    pub page: usize,
}

/// One crucible island: a GPU lane's population, trained a segment at a time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Island {
    pub name: String,
    pub segments: i64,
    pub generations: i64,
    pub running: String,
    pub failures: i64,
    /// The last yardstick read: (generation, share of games won).
    pub yard: Option<(i64, f64)>,
    pub curve: Vec<f64>,
    pub msteps: f64,
    /// The last ladder read: (generation, wins less losses against every
    /// milestone, and against the newest).
    pub ladder: Option<(i64, f64, f64)>,
}

/// crucible's milestones: one line of generations, each beating the one
/// before, whichever island found it (plan/crucible.json "_ladder").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ladder {
    /// (id, island, generation, margin in standard errors, when).
    pub milestones: Vec<(String, String, i64, f64, String)>,
    /// The milestones a segment is sent now, oldest first.
    pub rungs: Vec<String>,
    /// The newest lineage rows: (job, island, generation, against, margin, result).
    pub lineage: Vec<(String, String, i64, String, String, String)>,
    /// Champions kept, every segment's.
    pub kept: i64,
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
    /// What needs attention, not yet acknowledged: (id, time, kind, text).
    pub flags: Vec<(String, String, String, String)>,
    /// What the fleet has found, oldest first, needing no one: (time, kind,
    /// text). A candidate past both held-out reads, so far.
    pub progress: Vec<(String, String, String)>,
    /// Auto mode: candidates promote themselves (config "auto").
    pub auto: bool,
    /// Each candidate (w0034): true when it was read on today's defaults and
    /// may promote itself; false when on older defaults (a promotion since).
    pub current: Vec<(String, bool, String)>,
    /// The rotation: auto mode asks one of each kind in turn ("rotation").
    pub rotation: bool,
    /// crucible's islands, one a GPU lane (plan/crucible.json).
    pub islands: Vec<Island>,
    pub ladder: Ladder,
    /// The keeper, in a line: its pid, how long up, the last and next sync,
    /// and whether it holds this Mac awake.
    pub keeper: String,
    /// Push permission to the project's branch: (ok, "url branch", detail).
    /// Paul Accepts is a push, so a template must have it to land on main
    /// (without it a promotion self-modifies onto the local line).
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
    /// A promotion's candidate, and who asked ("auto" in auto mode).
    pub candidate: String,
    pub by: String,
    /// A promotion's machines and each one's state ("confirmed at ...",
    /// "building ...", "differs at ...").
    pub hosts: Vec<(String, String)>,
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
                flags: arr(q, "flags").iter().map(|f| (s(f, "id"), s(f, "t"), s(f, "kind"), s(f, "text"))).collect(),
                progress: arr(q, "progress").iter().map(|f| (s(f, "t"), s(f, "kind"), s(f, "text"))).collect(),
                auto: !matches!(q.get("auto"), Some(json::Value::Bool(false))),
                current: match q.get("candidate_state") {
                    Some(json::Value::Obj(m)) => m
                        .iter()
                        .map(|(k, v)| (k.clone(), matches!(v.get("current"), Some(json::Value::Bool(true))), s(v, "commit")))
                        .collect(),
                    _ => Vec::new(),
                },
                rotation: !matches!(q.get("rotation"), Some(json::Value::Bool(false))),
                islands: match q.get("crucible") {
                    Some(json::Value::Obj(m)) => m
                        .iter()
                        .filter(|(k, _)| !k.starts_with('_'))
                        .map(|(k, v)| Island {
                            name: k.clone(),
                            segments: n(v, "segments") as i64,
                            generations: n(v, "generations") as i64,
                            running: s(v, "running"),
                            failures: n(v, "failures") as i64,
                            yard: v.get("yard").and_then(|y| y.as_array()).and_then(|y| Some((y.first()?.as_num()? as i64, y.get(1)?.as_num()?))),
                            curve: arr(v, "yard_curve").iter().filter_map(|y| Some(y.as_array()?.get(1)?.as_num()?)).collect(),
                            msteps: n(v, "msteps"),
                            ladder: arr(v, "ladder_curve").last().and_then(|y| {
                                let y = y.as_array()?;
                                Some((y.first()?.as_num()? as i64, y.get(1)?.as_num()?, y.get(2)?.as_num()?))
                            }),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
                ladder: match q.get("crucible").and_then(|c| c.get("_ladder")) {
                    Some(l) => Ladder {
                        milestones: arr(l, "milestones")
                            .iter()
                            .map(|m| (s(m, "id"), s(m, "island"), n(m, "gen") as i64, n(m, "margin_se"), s(m, "made")))
                            .collect(),
                        rungs: arr(l, "rungs").iter().filter_map(|r| r.as_str().map(String::from)).collect(),
                        lineage: arr(l, "lineage")
                            .iter()
                            .map(|r| (s(r, "job"), s(r, "island"), s(r, "gen").parse().unwrap_or(0), s(r, "against"), s(r, "margin_se"), s(r, "result")))
                            .collect(),
                        kept: n(l, "kept") as i64,
                    },
                    None => Ladder::default(),
                },
                keeper: q
                    .get("keeper")
                    .filter(|k| k.get("pid").is_some())
                    .map(|k| {
                        let hm = |key: &str| { let v = s(k, key); if v.len() >= 16 { v[11..16].to_string() } else { "not yet".into() } };
                        format!(
                            "keeper pid {} since {}; last sync {}, next {}{}",
                            n(k, "pid") as i64,
                            hm("started"),
                            hm("last_sync"),
                            hm("next_sync"),
                            if k.get("awake").and_then(|a| a.as_num()).is_some() { "; keeping this Mac awake" } else { "" }
                        )
                    })
                    .unwrap_or_default(),
                push: q.get("push").filter(|p| p.get("ok").is_some()).map(|p| {
                    (p.get("ok").and_then(|b| b.as_bool()).unwrap_or(false), format!("{} {}", s(p, "url"), s(p, "branch")), s(p, "detail"))
                }),
                runs: arr(q, "runs")
                    .iter()
                    .map(|r| RunRow {
                        id: s(r, "id"),
                        kind: s(r, "kind"),
                        status: s(r, "status"),
                        detail: if s(r, "why").is_empty() { s(r, "detail") } else { s(r, "why") },
                        candidate: s(r, "candidate"),
                        by: s(r, "by"),
                        hosts: match r.get("hosts") {
                            Some(json::Value::Obj(m)) => m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect(),
                            _ => Vec::new(),
                        },
                    })
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

/// The pages of each chapter this file draws: the Queue's (Fleet, Runs) and
/// the Results' (Candidates, Found, Net, Flags), by page number.
const QUEUE_PAGES: [usize; 2] = [0, 1];
const RESULTS_PAGES: [usize; 4] = [5, 2, 3, 4];

/// (page number, name, what it shows). Candidates came after the rest and
/// keeps the number 5, so ORRERY_FRAME_PAGE and a saved page mean what they
/// meant.
const PAGES: [(usize, &str, &str); 6] = [
    (0, "Fleet", "each machine and its job"),
    (1, "Runs", "ask for a run, cancel, accept"),
    (2, "Found", "improvements, by variable and machine"),
    (5, "Candidates", "trials past both held-out reads: progress, nothing to answer"),
    (3, "Net", "the learned tank's weights"),
    (4, "Flags", "what needs attention; acknowledge it"),
];

/// Draw the Queue or the Results (`view`). Returns a view to move to.
pub fn draw(ui: &mut Ui, m: &Model, st: &mut State, view: nav::View) -> Option<nav::View> {
    let t = ui.t;
    let right = if m.fleet.is_empty() {
        String::new()
    } else {
        let up = m.machines.iter().filter(|x| x.up).count();
        format!("fleet {}  {} of {} machines answering", m.fleet, up, m.machines.len())
    };
    let results = view == nav::View::Results;
    let pages: &[usize] = if results { &RESULTS_PAGES } else { &QUEUE_PAGES };
    if !pages.contains(&st.page) {
        st.page = pages[0];
    }
    // The Flags key carries its count, its lamp red while any is
    // unacknowledged; the Candidates key its count, green.
    let (flags, found) = m.queue.as_ref().map(|q| (q.flags.len(), q.progress.iter().filter(|p| p.1 == "candidate").count())).unwrap_or((0, 0));
    let keys: Vec<nav::Key> = pages
        .iter()
        .map(|&p| {
            let name = PAGES.iter().find(|x| x.0 == p).map(|x| x.1).unwrap_or("");
            let (label, lamp) = match name {
                "Flags" if flags > 0 => (format!("Flags {}", flags), Some(t.alarm)),
                "Candidates" if found > 0 => (format!("Candidates {}", found), Some(t.up)),
                _ => (name.to_string(), None),
            };
            nav::Key { label, on: st.page == p, lamp }
        })
        .collect();
    let (rest, mut go, clicked) = nav::header(ui, view, true, &right, Some(&keys));
    if let Some(i) = clicked {
        st.page = pages[i];
    }
    if go == Some(view) {
        go = None;
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
    let sentence = match (&m.queue, results) {
        (Some(q), true) => results_headline(q),
        _ => sentence,
    };
    // Do it now, on the Results (Paul, 28 October: "it has a do it now
    // button to sync and push the values"): sync, then the strongest current
    // candidate promoted -- its tests and families on a branch, then main --
    // or, with none current, the strongest older ones read again. Two
    // presses: it can push to main.
    let room = if results && st.operator && m.queue.is_some() {
        let b = rect(body.right() - 300, y - 2, 300, 26);
        let label = if st.arm_now { "Press again: sync, promote, push" } else { "Do it now: sync, promote, push" };
        if ui.button(b, label, true) {
            if st.arm_now {
                st.ctl = Some("now".into());
                st.arm_now = false;
            } else {
                st.arm_now = true;
            }
        }
        310
    } else {
        0
    };
    ui.label_in(body.x, y, body.w - room, &sentence, &F10X20, t.ink);
    y += F10X20.height as i32 + 8;
    if results && !st.said.is_empty() {
        ui.label_in(body.x, y, body.w, &st.said, &F8X13, t.warn);
        y += F8X13.height as i32 + 4;
    }
    if let Some(q) = &m.queue {
        // The queue's band (its controls) on the Queue only; the newest
        // change in one line on both; then the page the column chose.
        if !results {
            y = draw_queue(ui, rect(body.x, y, body.w, 64), q, st) + 6;
        }
        y = draw_latest(ui, rect(body.x, y, body.w, 18), q) + 6;
        let what = PAGES.iter().find(|p| p.0 == st.page).map(|p| p.2).unwrap_or("");
        ui.label_in(body.x, y, body.w, what, &F8X13, t.dim);
        y += F8X13.height as i32 + 8;
        let page = rect(body.x, y, body.w, body.bottom() - y);
        match st.page {
            1 => draw_runs_page(ui, page, q, st),
            2 => draw_found_page(ui, page, q),
            4 => draw_flags_page(ui, page, q, st),
            5 => draw_candidates_page(ui, page, q, st),
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

/// The Results' sentence: what the fleet has found, in counts.
fn results_headline(q: &Queue) -> String {
    let found = q.progress.iter().filter(|p| p.1 == "candidate").count();
    let current = q.current.iter().filter(|c| c.1).count();
    let improvements: i64 = q.lead_vars.iter().map(|v| v.1).sum();
    format!(
        "{} found: {} candidates ({} on today's defaults), {} improvements; auto mode {}.",
        q.name,
        found,
        current,
        improvements,
        if q.auto { "on" } else { "off" }
    )
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
/// and every run asked for, with Cancel, and Accept or To main.
fn draw_runs_page(ui: &mut Ui, at: Rect, q: &Queue, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 3;
    // Each kind: its button, its command, what it does in the cycle (LR 118):
    // a search's result is adopted with its held-out reads; a measurement
    // runs once on each set of defaults; the rest are the learned tank's.
    const KINDS: [(&str, &str, &str); 13] = [
        ("spsa", "new spsa", "search: SPSA on the dodge; its result is adopted"),
        ("tune", "new tune", "search: (1 + lambda) on 15 weights; result adopted"),
        ("census", "new census trial=@best name=best", "the search's newest best, 55 seeds; passing, adopted"),
        ("parts", "new parts", "measure: each part of the brain off, against all on"),
        ("switches", "new switches", "measure: each switch that is off, alone"),
        ("roundrobin", "new roundrobin", "measure: every pair of profiles/*.weights"),
        ("record", "new record", "recordings for cloning, to the GPU store"),
        ("gauntlet", "new gauntlet", "net: single-decision tests of every net"),
        ("train", "new train", "net: evolution strategies, phase 1 (days)"),
        ("school", "new school", "net: the learned tank's curriculum (days)"),
        ("clips", "new clips host=mac", "net: a short GIF of each milestone net (this Mac)"),
        ("families", "new families", "every candidate's families again (auto after a promotion)"),
        ("promote", "new promote candidate=@strongest", "the strongest current candidate, now"),
    ];
    let half = at.w / 2 - 6;
    let mut y = at.y;
    // The rotation: what auto mode asks by itself, one of each in turn.
    let rot = if q.auto && q.rotation {
        "Rotation ON, one of each in turn. brain: spsa tune census parts switches roundrobin record. net: gauntlet train school"
    } else if q.auto {
        "Rotation OFF: only the standing search, and what is asked here."
    } else {
        "Auto mode is off: no rotation; only the standing search, and what is asked here."
    };
    let rbw = if st.operator && q.auto { 150 } else { 0 };
    ui.label_in(at.x, y, at.w - rbw - 10, rot, &F8X13, if q.auto && q.rotation { t.up } else { t.dim });
    if rbw > 0 && ui.button(rect(at.right() - rbw, y - 3, rbw, lh + 2), if q.rotation { "Turn rotation off" } else { "Turn rotation on" }, true) {
        st.ctl = Some(if q.rotation { "rotation off".into() } else { "rotation on".into() });
    }
    y += lh + 8;
    let top = y;
    ui.label(at.x, y, if st.operator { "Ask for a run (it goes first)" } else { "Kinds of run (an operator can ask for one)" }, &F8X13, t.ink);
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
    let mut y = top;
    ui.label(rx, y, "Runs (r: the rotation's, a: auto mode's)", &F8X13, t.ink);
    y += lh;
    // Whether this machine can push decides how a promotion lands: Paul
    // Accepts (pushed to main), or it self-modifies (the local line).
    let (pushline, pc) = match &q.push {
        Some((true, at, _)) => (format!("push: ok to {}; with auto mode on, Paul Accepts each promotion", at), t.up),
        Some((false, at, why)) => (format!("push: not permitted to {} ({}); promotions self-modify until resolved", at, why), t.warn),
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
        // A self-modified promotion waits on nobody, but it can be sent to
        // main by hand (the local line, pushed).
        let ready = r.kind == "promote" && (r.status == "prepared" || r.status == "self-modified");
        let cw = if st.operator && (live || ready) { 70 } else { 0 };
        let colour = if r.status == "failed" { t.alarm } else if r.status == "self-modified" { t.warn } else if ready { t.up } else if live { t.ink } else { t.dim };
        let by = match r.by.as_str() {
            "rotation" => " r",
            "auto" => " a",
            _ => "",
        };
        let line = if r.detail.is_empty() { format!("{}{} {} {}", r.id, by, r.kind, r.status) } else { format!("{}{} {} {}: {}", r.id, by, r.kind, r.status, r.detail) };
        ui.label_in(rx, y, rw - cw - 6, &line, &F8X13, colour);
        if cw > 0 && live && ui.button(rect(at.right() - cw, y - 2, cw, lh), "Cancel", true) {
            st.ctl = Some(format!("cancel {}", r.id));
        }
        if cw > 0 && ready && ui.button(rect(at.right() - cw, y - 2, cw, lh), if r.status == "self-modified" { "To main" } else { "Accept" }, true) {
            st.ctl = Some(format!("approve {}", r.id));
        }
        y += lh + 2;
    }
}

/// The Flags page: what the fleet raised for a person to see (a candidate
/// past both reads, the local line to resolve, a run done or failed,
/// a machine lost, the breaker, push permission), newest first; an
/// operator acknowledges each, or all.
fn draw_flags_page(ui: &mut Ui, at: Rect, q: &Queue, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 6;
    let mut y = at.y;
    if q.flags.is_empty() {
        ui.label_in(at.x, y, at.w, "Nothing needs attention: every flag is acknowledged.", &F8X13, t.dim);
        return;
    }
    ui.label(at.x, y, &format!("{} flag(s) not acknowledged, newest first", q.flags.len()), &F8X13, t.ink);
    if st.operator && ui.button(rect(at.right() - 150, y - 4, 150, 22), "Acknowledge all", true) {
        st.ctl = Some("ack all --by operator".into());
    }
    y += lh + 4;
    let kw = F8X13.measure("candidate  ") as i32;
    for (id, when, kind, text) in q.flags.iter().rev() {
        if y + lh > at.bottom() {
            break;
        }
        let bw = if st.operator { 110 } else { 0 };
        ui.label(at.x, y, if when.len() >= 16 { &when[11..16] } else { when }, &F8X13, t.dim);
        ui.label(at.x + 52, y, kind, &F8X13, if kind == "breaker" || kind == "push" || kind == "families" { t.alarm } else if kind == "resolve" { t.warn } else { t.up });
        ui.label_in(at.x + 52 + kw, y, at.w - 52 - kw - bw - 10, text, &F8X13, t.ink);
        if bw > 0 && ui.button(rect(at.right() - bw, y - 3, bw, lh - 2), "Acknowledge", true) {
            st.ctl = Some(format!("ack {} --by operator", id));
        }
        y += lh;
    }
}

/// The Candidates page: every trial that passed both held-out reads, newest
/// first. News of progress: nothing here waits for anyone (Paul, 28 October:
/// "I'm going to always accept them. Just make them visible").
fn draw_candidates_page(ui: &mut Ui, at: Rect, q: &Queue, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 6;
    let mut y = at.y;
    // Auto mode, and its switch.
    let mode = if q.auto {
        "Auto mode ON: a run's result gets its held-out reads; the strongest current candidate promotes itself; Paul Accepts it when this machine can push, else it self-modifies; every machine's build is checked for its values."
    } else {
        "Auto mode OFF: a promotion is asked for by hand; prepared, it self-modifies (the local line) until it is sent to main."
    };
    let bw = if st.operator { 150 } else { 0 };
    ui.label_in(at.x, y, at.w - bw - 10, mode, &F8X13, if q.auto { t.up } else { t.dim });
    if st.operator && ui.button(rect(at.right() - bw, y - 4, bw, 22), if q.auto { "Turn auto off" } else { "Turn auto on" }, true) {
        st.ctl = Some(if q.auto { "auto off".into() } else { "auto on".into() });
    }
    y += lh + 4;
    // Promotions, newest first, a line each. The newest approved one shows
    // each machine's confirmation that its values reached it. A failure
    // older than an approved promotion is history, not an alarm: the cause
    // was met and a later promotion landed (r0009 failed on the
    // Tactics::default() test LR 119 fixed; r0019 landed after).
    let proms: Vec<&RunRow> = q.runs.iter().filter(|r| r.kind == "promote").rev().collect();
    let last_ok = proms.iter().find(|r| r.status == "approved").map(|r| r.id.clone());
    if !proms.is_empty() {
        ui.label(at.x, y, "Promotions (newest first)", &F8X13, t.ink);
        y += lh;
        for r in proms.iter().take(4) {
            let superseded = r.status == "failed" && last_ok.as_ref().map(|ok| ok.as_str() > r.id.as_str()).unwrap_or(false);
            let c = match r.status.as_str() {
                "approved" => t.up,
                "self-modified" => t.warn,
                "failed" if superseded => t.dim,
                "failed" => t.alarm,
                _ => t.warn,
            };
            let who = match r.by.as_str() { "auto" => " (auto)", "now" => " (do it now)", _ => "" };
            let detail = if superseded { format!("{}; superseded by {}", r.detail, last_ok.clone().unwrap_or_default()) } else { r.detail.clone() };
            ui.label_in(at.x, y, at.w, &format!("{}{}  {}  {}: {}", r.id, who, r.candidate, r.status, detail), &F8X13, c);
            y += lh;
            if Some(&r.id) == last_ok.as_ref() && !r.hosts.is_empty() {
                // The machines on one line: ok where the build has the values.
                let mut x = at.x + 16;
                for (host, state) in &r.hosts {
                    let ok = state.starts_with("confirmed");
                    let bad = state.starts_with("differs") || state.contains("failed");
                    let mark = if ok { "ok" } else if bad { "!!" } else { ".." };
                    let col = if ok { t.up } else if bad { t.alarm } else { t.dim };
                    x = ui.label(x, y, mark, &F8X13, col) + 4;
                    x = ui.label(x, y, host, &F8X13, t.ink) + 14;
                    if x > at.right() - 120 {
                        break;
                    }
                }
                y += lh;
            }
        }
        y += 4;
    }
    let cands: Vec<&(String, String, String)> = q.progress.iter().filter(|p| p.1 == "candidate").collect();
    if cands.is_empty() {
        ui.label_in(at.x, y, at.w, "No candidate yet: none has passed both held-out reads.", &F8X13, t.dim);
        return;
    }
    let first = cands.first().map(|p| p.0.as_str()).unwrap_or("");
    let since = if first.len() >= 16 { first[..16].replace('T', " ") } else { first.to_string() };
    // Each candidate's standing: promoted (it is the defaults), current (read
    // on today's defaults: it may promote itself), older (read before a
    // promotion moved the defaults: auto mode reads the strongest again).
    let standing = |id: &str| -> (u8, String) {
        if let Some(r) = q.runs.iter().find(|r| r.kind == "promote" && r.status == "approved" && r.candidate.rsplit('/').next() == Some(id)) {
            return (1, format!("promoted ({})", r.id));
        }
        match q.current.iter().find(|(k, _, _)| k == id) {
            Some((_, true, _)) => (2, "current".to_string()),
            Some((_, false, at)) => (3, format!("older ({})", if at.is_empty() { "?" } else { at })),
            None => (3, "not read".to_string()),
        }
    };
    let rows: Vec<(&(String, String, String), u8, String)> = cands
        .iter()
        .rev()
        .map(|p| {
            let id = p.2.split_whitespace().next().unwrap_or("");
            let (k, tag) = standing(id);
            (*p, k, tag)
        })
        .collect();
    let count = |k: u8| rows.iter().filter(|r| r.1 == k).count();
    // The filter, by kind: keys with their counts.
    let filters: [(u8, String); 4] = [
        (0, format!("All {}", rows.len())),
        (1, format!("Promoted {}", count(1))),
        (2, format!("Current {}", count(2))),
        (3, format!("Older {}", count(3))),
    ];
    let mut fx = at.x;
    for (k, label) in &filters {
        let w = F8X13.measure(label) as i32 + 26;
        let r = rect(fx, y - 3, w, 22);
        let on = st.cand_filter == *k;
        if t.metal {
            ui.c.metal(r.x, r.y, r.w, r.h, if on { crate::draw::Finish::Pressed } else { crate::draw::Finish::Button }, 6);
            ui.label(r.x + 13, r.y + 4 + if on { 1 } else { 0 }, label, &F8X13, t.engrave);
        } else {
            ui.panel(r, if on { t.tile } else { t.panel });
            ui.outline(r, if on { t.accent } else { t.line });
            ui.label(r.x + 13, r.y + 4, label, &F8X13, if on { t.ink } else { t.dim });
        }
        if ui.button_hit(r) {
            st.cand_filter = *k;
            st.cand_off = 0;
        }
        fx += w + 6;
    }
    let note = format!(
        "since {}, newest first. {}",
        since,
        if q.auto { "The strongest current one promotes itself; with none current, the strongest older ones are read again." } else { "A candidate is promoted from Runs (promote)." }
    );
    ui.label_in(fx + 8, y, at.right() - fx - 8, &note, &F8X13, t.dim);
    y += lh + 6;
    let shown: Vec<&(&(String, String, String), u8, String)> = rows.iter().filter(|r| st.cand_filter == 0 || r.1 == st.cand_filter).collect();
    // Columns: when; the trial and its two reads; its standing; what it moves.
    let when_w = F8X13.measure("10-28 00:00  ") as i32;
    let head_w = F8X13.measure("w0000  +0.00  +0.00   ") as i32;
    let state_w = F8X13.measure("older (8f852ac)  ") as i32;
    let box_ = rect(at.x, y, at.w, at.bottom() - y);
    let mut off = st.cand_off;
    let (area, first_row, fit) = ui.scroll_list(box_, shown.len(), lh, &mut off);
    st.cand_off = off;
    let mut y = area.y;
    for ((when, _kind, text), k, tag) in shown.iter().skip(first_row).take(fit).map(|r| (r.0, r.1, &r.2)) {
        // "w0024 passed both held-out reads (+2.15, +2.61): castle_inside 8, ..."
        let (head, moved) = text.split_once(": ").unwrap_or((text.as_str(), ""));
        let id = head.split_whitespace().next().unwrap_or("");
        let reads = head.rsplit_once('(').map(|(_, r)| r.trim_end_matches(')').replace(',', " ")).unwrap_or_default();
        let w = if when.len() >= 16 { format!("{} {}", &when[5..10], &when[11..16]) } else { when.to_string() };
        let cur = k != 3;
        ui.label(area.x, y, &w, &F8X13, t.dim);
        ui.label(area.x + when_w, y, &format!("{}  {}", id, reads), &F8X13, if cur { t.up } else { t.dim });
        ui.label(area.x + when_w + head_w, y, tag, &F8X13, if cur { t.up } else { t.dim });
        let mx = area.x + when_w + head_w + state_w;
        ui.label_in(mx, y, area.right() - mx, moved, &F8X13, if cur { t.ink } else { t.dim });
        y += lh;
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
    if !q.keeper.is_empty() {
        ui.label_in(at.x, ly, bw, &q.keeper, &F8X13, t.dim);
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
        let stop = if st.arm_stop { "Press: collect results and stop now" } else { "Collect results and stop now" };
        if running_keeper && ui.button(rect(x, at.y, 280, 26), stop, true) {
            st.ctl = Some("stop-now".into());
            st.arm_stop = false;
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

/// What sense `i` of the 143 is (treadcore-brain senses.rs: the compass's
/// 8 sectors of 11 channels, the body, the instruments, the periscope's
/// contacts, the hints, the shell and air contacts).
pub fn sense_name(i: usize) -> String {
    const SECTOR: [&str; 8] = ["ahead", "ahead right", "right", "behind right", "behind", "behind left", "left", "ahead left"];
    const CHANNEL: [&str; 11] = [
        "hostile tank", "allied tank", "hostile pillbox", "neutral pillbox", "our pillbox", "depot to take",
        "our depot", "shell coming", "ground stops us", "cover (forest)", "a friend's call",
    ];
    const REST: [&str; 55] = [
        "armour", "shells", "fuel", "reloading", "speed", "in a boat", "pillboys out", "pillboy material",
        "recon drones in bay", "interceptors in bay", "strike drones in bay", "longshots in bay",
        "map x", "map y", "heading sine", "heading cosine", "nearest map edge", "hit just taken",
        "enemy bearing sine", "enemy bearing cosine", "enemy strength", "enemy closing", "enemy pointing at us", "enemy in gun reach",
        "thing to take sine", "thing to take cosine", "thing to take strength", "our depot sine", "our depot cosine", "our depot strength",
        "call sine", "call cosine", "call strength", "call: help", "call: attack", "call: take",
        "hint: economy", "hint: fortify", "hint: connect", "hint: expand", "hint: defend", "hint: the odds", "hint: base threatened",
        "hint: errand sector", "hint: errand strength",
        "shell impact sine", "shell impact cosine", "shell impact near", "shell lands soon", "inside the splash",
        "drone sine", "drone cosine", "drone near", "drone diving (strike)", "our interceptor bubble",
    ];
    if i < 88 {
        format!("compass {} ({}): {}", i / 11, SECTOR[i / 11], CHANNEL[i % 11])
    } else {
        REST.get(i - 88).map(|s| s.to_string()).unwrap_or_else(|| format!("sense {}", i))
    }
}

/// What score `o` of the 47 is: a head's choice (the heads' sizes).
pub fn score_name(o: usize, heads: &[(String, i64)]) -> String {
    let mut at = 0usize;
    for (h, n) in heads {
        if o < at + *n as usize {
            return format!("{} choice {} of {}", h, o - at + 1, n);
        }
        at += *n as usize;
    }
    format!("score {}", o)
}

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
    // Where each matrix was drawn: (x, y, w, h, rows, cols), for the marks
    // the bars below put on them.
    let mut boxes: Vec<(i32, i32, i32, i32, i32, i32)> = Vec::new();
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
        boxes.push((x, gy, gw, gh, rows, cols));
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

    // Every sense and every score, a bar each (Paul, 28 October: "can we
    // show each variable with a bar ... expand this for all values"). A
    // sense's bar is how much the net listens to it: the sum of its column
    // of matrix 1, in size. A score's is how much reaches it: its row of
    // matrix 3. Pointing at a bar names it, says how it is transformed on
    // its way, and marks its column or row on the matrix above.
    let mut mark: Option<(usize, bool, usize)> = None; // (layer, column?, index)
    let mut bar_hover: Option<String> = None;
    let chart_h = 46;
    let first = net.layers.first();
    let last = net.layers.last();
    let col_sums: Vec<f64> = first
        .filter(|l| l.cells.len() as i64 == l.outputs && l.cells.first().map(|r| r.len() as i64) == Some(l.inputs))
        .map(|l| (0..l.inputs as usize).map(|c| l.cells.iter().map(|r| r[c].abs()).sum()).collect())
        .unwrap_or_default();
    let row_sums: Vec<f64> = last
        .filter(|l| l.cells.len() as i64 == l.outputs && l.cells.first().map(|r| r.len() as i64) == Some(l.inputs))
        .map(|l| l.cells.iter().map(|r| r.iter().map(|v| v.abs()).sum()).collect())
        .unwrap_or_default();
    let hidden = net.sizes.get(1).copied().unwrap_or(0);
    let hidden2 = net.sizes.get(net.sizes.len().saturating_sub(2)).copied().unwrap_or(0);
    for (which, sums) in [(0usize, &col_sums), (1usize, &row_sums)] {
        if sums.is_empty() || y + chart_h + 2 * F8X13.height as i32 > at.bottom() {
            continue;
        }
        let title = if which == 0 {
            format!("Every sense ({}), by how much the net listens to it: its column of matrix 1", sums.len())
        } else {
            format!("Every score ({}), by how much reaches it: its row of matrix 3", sums.len())
        };
        ui.label_in(at.x, y, at.w, &title, &F8X13, t.ink);
        y += F8X13.height as i32 + 3;
        let n = sums.len() as i32;
        let bw = (at.w / n).max(2);
        let top = sums.iter().cloned().fold(1e-9, f64::max);
        let groups = if which == 0 { &net.input_groups } else { &net.output_groups };
        let (px, py) = ui.pointer();
        for (k, v) in sums.iter().enumerate() {
            let bx = at.x + k as i32 * bw;
            let h = ((chart_h - 2) as f64 * v / top).round() as i32;
            let gi = groups.iter().position(|g| k as i64 >= g.1 && k as i64 <= g.2).unwrap_or(0);
            let base = if gi % 2 == 0 { t.up } else { t.accent };
            let hot = px >= bx && px < bx + bw && py >= y && py < y + chart_h;
            ui.c.rect(bx, y + chart_h - h, (bw - 1).max(1), h.max(1), if hot { t.ink } else { base });
            if hot {
                mark = Some((if which == 0 { 0 } else { net.layers.len() - 1 }, which == 0, k));
                bar_hover = Some(if which == 0 {
                    format!(
                        "sense {}: {}. Read by all {} units of layer 1 (column {} of matrix 1, sum of sizes {:.2}); each unit adds it, weighted, to the rest and clips below 0; layers 2 and 3 carry it to the 47 scores.",
                        k, sense_name(k), hidden, k, v
                    )
                } else {
                    format!(
                        "score {}: {}. Row {} of matrix 3 (sum of sizes {:.2}): the {} units of layer 2, weighted and summed; the highest score of its head is what the net does.",
                        k, score_name(k, &net.heads), k, v, hidden2
                    )
                });
            }
        }
        ui.c.rect(at.x, y + chart_h, n * bw, 1, t.line);
        // The groups' names under their bars.
        for g in groups.iter() {
            let gx = at.x + g.1 as i32 * bw;
            let gw = ((g.2 - g.1 + 1) as i32 * bw).max(1);
            ui.c.rect(gx, y + chart_h + 2, gw - 2, 2, t.dim);
            ui.label_in(gx, y + chart_h + 5, gw - 2, &g.0, &F8X13, t.dim);
        }
        y += chart_h + 6 + F8X13.height as i32 + 6;
    }
    // The bar under the pointer, marked on its matrix: a column for a sense,
    // a row for a score.
    if let Some((li, col, k)) = mark {
        if let Some(&(bx, by, bw, bh, rows, cols)) = boxes.get(li) {
            if col {
                let mx = bx + (k as i32 * bw) / cols.max(1);
                ui.c.rect(mx, by - 4, 1, bh + 8, t.warn);
            } else {
                let my = by + (k as i32 * bh) / rows.max(1);
                ui.c.rect(bx - 4, my, bw + 8, 1, t.warn);
            }
        }
    }
    // What the bar under the pointer is, and how it is transformed.
    if let Some(h) = &bar_hover {
        let hy = y;
        ui.label_in(at.x, hy, at.w, h, &F8X13, t.warn);
        y += F8X13.height as i32 + 8;
    }

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
    fn crucible_s_ladder_is_read_and_is_not_an_island() {
        let doc = r#"{"queue":{"state":"running","crucible":{
            "calber-cuda":{"segments":3,"generations":1500,"running":"x-calber-cuda-0003","failures":0,
                           "yard":[1500,0.98],"yard_curve":[[1500,0.98]],"ladder_curve":[[1490,0.1,-0.2],[1500,0.25,0.12]],"msteps":529},
            "_ladder":{"milestones":[{"id":"M0001-calber-cuda-g1000","island":"calber-cuda","gen":1000,"margin_se":0,"made":"t"}],
                       "rungs":["M0001-calber-cuda-g1000"],"kept":4,
                       "lineage":[{"job":"x-calber-cuda-0003","island":"calber-cuda","gen":"1500","against":"M0001-calber-cuda-g1000",
                                   "margin_se":"5.1","result":"milestone M0002"}]}}}}"#;
        let m = Model::parse(doc, "test");
        let q = m.queue.as_ref().expect("a queue");
        assert_eq!(q.islands.len(), 1, "_ladder is not an island");
        assert_eq!(q.islands[0].ladder, Some((1500, 0.25, 0.12)));
        assert_eq!(q.ladder.milestones[0].0, "M0001-calber-cuda-g1000");
        assert_eq!(q.ladder.kept, 4);
        assert_eq!(q.ladder.lineage[0].2, 1500);
    }

    #[test]
    fn no_command_and_bad_output_each_say_why() {
        assert!(Model::parse("", "").why.contains("--training-cmd"));
        assert!(Model::parse("nope", "x").why.contains("did not print JSON"));
    }
}
