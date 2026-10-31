//! The Projects chapter: what the fleet works on, and which machine runs
//! which (Paul, 28 October: "incorporate orrery and crucible into its second
//! job. we want to complement and use our fleet").
//!
//! Two projects share grove:
//! - **treadcore**, the tank game's brain, measured by census on the CPUs:
//!   its queue, its candidates, auto mode;
//! - **crucible**, a self-learning loop that runs entirely on a GPU
//!   (`~/code/crucible`): its runs, read from its `runs/` folder.
//!
//! Then a row per machine: its cores and card, and what each project has on
//! it. NOTHING HERE IS INVENTED: treadcore's lines are the queue's status, and
//! crucible's are its run files. crucible is not in the queue yet, and the
//! page says so rather than drawing a lamp it has not earned.

use crate::font::{F10X20, F8X13};
use crate::nav;
use crate::training::Model;
use crate::ui::{rect, Light, Rect, Ui};
use std::time::{Duration, Instant};

/// One crucible run, from its TSV: one line a generation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrucibleRun {
    pub name: String,
    pub gens: usize,
    /// The champion's last read against the yardstick: (generation, wins).
    pub yard: Option<(usize, f64)>,
    /// Tank-steps a second, the run's best generation, millions.
    pub msteps: f64,
}

/// crucible as its folder says, read again every 15 s.
#[derive(Debug, Clone, Default)]
pub struct Crucible {
    pub dir: String,
    pub found: bool,
    pub runs: Vec<CrucibleRun>,
    pub read_at: Option<Instant>,
}

impl Crucible {
    pub fn new() -> Crucible {
        let dir = std::env::var("ORRERY_CRUCIBLE").unwrap_or_else(|_| format!("{}/code/crucible", std::env::var("HOME").unwrap_or_default()));
        Crucible { dir, ..Default::default() }
    }

    pub fn refresh(&mut self) {
        if self.read_at.map(|t| t.elapsed() < Duration::from_secs(15)).unwrap_or(false) {
            return;
        }
        self.read_at = Some(Instant::now());
        self.found = std::path::Path::new(&format!("{}/src/crucible.cu", self.dir)).exists();
        let mut runs = Vec::new();
        if let Ok(rd) = std::fs::read_dir(format!("{}/runs", self.dir)) {
            let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "tsv").unwrap_or(false)).collect();
            paths.sort();
            for p in paths {
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                runs.push(parse_run(&name, &text));
            }
        }
        self.runs = runs;
    }
}

/// A run's TSV: gen, best, mean, play_s, steps_per_s, net_gflops, hall,
/// yard_win, yard_draw, yard_loss; a yardstick of -1 means not read.
pub fn parse_run(name: &str, text: &str) -> CrucibleRun {
    let mut r = CrucibleRun { name: name.to_string(), ..Default::default() };
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 8 {
            continue;
        }
        let g: usize = f[0].parse().unwrap_or(0);
        r.gens = r.gens.max(g + 1);
        let sps: f64 = f[4].parse().unwrap_or(0.0);
        r.msteps = r.msteps.max(sps / 1e6);
        if let Ok(y) = f[7].parse::<f64>() {
            if y >= 0.0 {
                r.yard = Some((g + 1, y));
            }
        }
    }
    r
}

/// What the page asks of the coordinator, and what it last said.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub operator: bool,
    /// A button pressed once: the next press does it.
    pub armed: Option<&'static str>,
    /// `machines.py cuda-lanes ...`, taken and run by main.rs.
    pub ask: Option<String>,
    /// A fleet-queue.py command, taken and run by main.rs: a project or a
    /// machine's buttons.
    pub ctl: Option<String>,
    /// A machine's Stop now, pressed once.
    pub armed_machine: Option<String>,
    pub said: String,
}

/// The GPU lanes in the queue (NAME-cuda) and the machines with a card.
fn lanes(m: &Model) -> (Vec<String>, usize) {
    let lanes = m.queue.as_ref().map(|q| q.lanes.iter().filter(|l| l.host.ends_with("-cuda")).map(|l| l.host.clone()).collect()).unwrap_or_default();
    (lanes, m.machines.iter().filter(|x| x.gpu.is_some()).count())
}

pub fn draw(ui: &mut Ui, m: &Model, c: &mut Crucible, st: &mut State) -> Option<nav::View> {
    let t = ui.t;
    c.refresh();
    let (cuda, gpus) = lanes(m);
    let right = format!("{} machines, {} with a GPU, {} in crucible", m.machines.len(), gpus, cuda.len());
    let (rest, go, _) = nav::header(ui, nav::View::Projects, true, &right, None);
    let body = rest.inset(10);
    let mut y = body.y;
    ui.label_in(body.x, y, body.w - 330, "Two projects share the fleet: treadcore on the CPUs, crucible on the GPUs.", &F10X20, t.ink);
    // The button: every GPU machine a crucible lane, or none. Two presses,
    // as the queue's stop: it changes what the fleet runs.
    if st.operator {
        let (word, verb, key) = if cuda.len() < gpus {
            ("Add the GPUs to crucible", "add", "add")
        } else {
            ("Take crucible off the GPUs", "remove", "remove")
        };
        let armed = st.armed == Some(key);
        let label = if armed { "Press again to confirm" } else { word };
        let b = rect(body.right() - 230, y - 2, 230, 26);
        if ui.button(b, label, true) {
            if armed {
                st.ask = Some(format!("cuda-lanes {} all", verb));
                st.armed = None;
            } else {
                st.armed = Some(key);
            }
        }
    }
    y += F10X20.height as i32 + 6;
    let note = if !st.said.is_empty() {
        st.said.clone()
    } else if cuda.is_empty() {
        "crucible runs on a GPU lane of its own (NAME-cuda), beside the machine's treadcore lane: the button adds one to each machine with a card and CUDA.".to_string()
    } else {
        format!("crucible lanes: {}. Each is an island: a segment at a time, swapping champions with the others.", cuda.join(", "))
    };
    ui.label_in(body.x, y, body.w, &note, &F8X13, if st.armed.is_some() { t.warn } else { t.dim });
    y += F8X13.height as i32 + 8;

    let card_h = 150;
    let half = (body.w - 10) / 2;
    draw_treadcore(ui, rect(body.x, y, half, card_h), m);
    draw_crucible(ui, rect(body.x + half + 10, y, body.w - half - 10, card_h), m, c);
    y += card_h + 14;

    let y = match m.queue.as_ref().filter(|q| !q.roster.is_empty()) {
        Some(q) => draw_project_buttons(ui, rect(body.x, y, body.w, body.bottom() - y), q, st),
        None => y,
    };
    let y = draw_matrix(ui, rect(body.x, y, body.w, body.bottom() - y), m, c, st);
    if m.queue.as_ref().map(|q| !q.islands.is_empty()).unwrap_or(false) {
        draw_ladder(ui, rect(body.x, y + 10, body.w, body.bottom() - y - 10), m);
    }
    go
}

/// crucible's milestones, a chain oldest to newest, and the newest
/// champions with what became of each (the lineage).
fn draw_ladder(ui: &mut Ui, at: Rect, m: &Model) {
    let t = ui.t;
    let Some(q) = &m.queue else { return };
    let l = &q.ladder;
    let lh = F8X13.height as i32;
    if at.h < 3 * lh {
        return;
    }
    let tw = ui.label(at.x, at.y, "crucible's ladder", &F8X13, t.accent);
    let note = format!(
        "milestones, each beating the one before: {}; {} champions kept; a segment meets {}",
        l.milestones.len(),
        l.kept,
        l.rungs.len()
    );
    ui.label_in(tw + 12, at.y, at.right() - tw - 12, &note, &F8X13, t.dim);
    let mut y = at.y + lh + 8;
    if l.milestones.is_empty() {
        ui.label_in(
            at.x + 10,
            y,
            at.w - 20,
            "none yet: the first exam made sets milestone 1, and from then on a champion must beat the newest by two standard errors",
            &F8X13,
            t.dim,
        );
        return;
    }
    // The chain: as many of the newest as fit, oldest on the left.
    let bw = 132;
    let gap = 18;
    let fit = ((at.w + gap) / (bw + gap)).max(1) as usize;
    let shown = &l.milestones[l.milestones.len().saturating_sub(fit)..];
    let bh = 3 * lh + 14;
    let mut x = at.x;
    for (i, (id, island, gen, margin, _)) in shown.iter().enumerate() {
        let b = rect(x, y, bw, bh);
        ui.panel(b, t.tile);
        ui.outline(b, if l.rungs.contains(id) { t.accent } else { t.line });
        let short = id.split('-').next().unwrap_or(id);
        ui.label(b.x + 8, b.y + 5, short, &F8X13, t.ink);
        if l.rungs.contains(id) {
            ui.light(b.right() - 12, b.y + 5 + lh / 2, 3, Light::Full, t.up);
        }
        ui.label_in(b.x + 8, b.y + 5 + lh + 3, bw - 16, &format!("{} g{}", island.trim_end_matches("-cuda"), gen), &F8X13, t.dim);
        let m = if i == 0 && l.milestones.len() == shown.len() { "first".to_string() } else { format!("+{:.1} se", margin) };
        ui.label_in(b.x + 8, b.y + 5 + 2 * (lh + 3), bw - 16, &m, &F8X13, t.up);
        if i + 1 < shown.len() {
            let ay = y + bh / 2;
            ui.panel(rect(b.right() + 3, ay, gap - 6, 1), t.dim);
            ui.panel(rect(b.right() + gap - 6, ay - 2, 2, 5), t.dim);
        }
        x += bw + gap;
    }
    y += bh + 10;
    // The lineage, newest first.
    for (job, island, gen, against, margin, result) in l.lineage.iter().rev() {
        if y + lh > at.bottom() {
            break;
        }
        let col = if result.starts_with("milestone") { t.up } else { t.dim };
        ui.label_in(at.x + 10, y, 170, job, &F8X13, t.ink);
        ui.label_in(at.x + 185, y, 130, &format!("{} g{}", island.trim_end_matches("-cuda"), gen), &F8X13, t.ink);
        let m = margin.parse::<f64>().map(|v| format!("{:+.1} se vs {}", v, against.split('-').next().unwrap_or(""))).unwrap_or_else(|_| "no exam".into());
        ui.label_in(at.x + 320, y, 140, &m, &F8X13, t.ink);
        ui.label_in(at.x + 465, y, at.w - 475, result, &F8X13, col);
        y += lh + 3;
    }
}

fn card(ui: &mut Ui, at: Rect, name: &str, lamp: (Light, crate::draw::Rgb, &str)) -> i32 {
    let t = ui.t;
    ui.panel(at, t.tile);
    ui.outline(at, t.line);
    let x = at.x + 10;
    let nx = ui.label(x, at.y + 8, name, &F10X20, t.ink);
    ui.light(nx + 14, at.y + 8 + F10X20.height as i32 / 2, 4, lamp.0, lamp.1);
    ui.label_in(nx + 24, at.y + 12, at.right() - nx - 30, lamp.2, &F8X13, lamp.1);
    at.y + 8 + F10X20.height as i32 + 6
}

fn lines(ui: &mut Ui, at: Rect, mut y: i32, rows: &[(String, crate::draw::Rgb)]) {
    for (s, c) in rows {
        if y + F8X13.height as i32 > at.bottom() - 4 {
            break;
        }
        ui.label_in(at.x + 10, y, at.w - 20, s, &F8X13, *c);
        y += F8X13.height as i32 + 3;
    }
}

fn draw_treadcore(ui: &mut Ui, at: Rect, m: &Model) {
    let t = ui.t;
    let Some(q) = &m.queue else {
        let y = card(ui, at, "treadcore", (Light::Hollow, t.warn, "no queue: start orrery with --queue"));
        lines(ui, at, y, &[("the tank game's brain, measured by census on the CPUs".into(), t.dim)]);
        return;
    };
    let mut tot = [0i64; 5];
    for (_, k) in &q.counts {
        for i in 0..5 {
            tot[i] += k[i];
        }
    }
    let found = q.progress.iter().filter(|p| p.1 == "candidate").count();
    let current = q.current.iter().filter(|c| c.1).count();
    let paused = q.projects.iter().any(|(n, p)| n == "treadcore" && *p);
    let state = format!("in queue {}: {}", q.name, if !q.keeper_up() { "no keeper: nothing runs" } else if paused { "paused" } else if q.adding { "adding work" } else { "not adding" });
    let lamp = if q.keeper_up() && !paused { (Light::Full, t.up, &state[..]) } else { (Light::Hollow, t.warn, &state[..]) };
    let y = card(ui, at, "treadcore", lamp);
    // What the buttons left on it, not what is on the network.
    let on: Vec<&str> = q.roster.iter().filter(|r| r.joined && !r.paused && r.on.iter().any(|(p, v)| p == "treadcore" && *v == Some(true))).map(|r| r.name.as_str()).collect();
    let cpus = if q.roster.is_empty() { format!("runs on all {} machines (every one has CPUs)", m.machines.len()) } else { format!("set to run on {}: {}", on.len(), on.join(", ")) };
    lines(
        ui,
        at,
        y,
        &[
            ("the tank game's brain, measured by census on the CPUs".into(), t.dim),
            (format!("{} queued, {} running, {} done, {} failed", tot[0], tot[1], tot[2], tot[3]), t.ink),
            (format!("auto mode {}, rotation {}", if q.auto { "on" } else { "off" }, if q.rotation { "on" } else { "off" }), t.ink),
            (format!("{} candidates, {} on today's defaults", found, current), if found > 0 { t.up } else { t.ink }),
            (cpus, t.dim),
        ],
    );
}

fn draw_crucible(ui: &mut Ui, at: Rect, m: &Model, c: &Crucible) {
    let t = ui.t;
    let islands = m.queue.as_ref().map(|q| &q.islands[..]).unwrap_or(&[]);
    if !islands.is_empty() {
        let running = islands.iter().filter(|i| !i.running.is_empty()).count();
        let state = format!("in the queue: {} island(s), {} running", islands.len(), running);
        let y = card(ui, at, "crucible", (Light::Full, t.up, &state));
        let mut rows: Vec<(String, crate::draw::Rgb)> = vec![("on the GPUs; ladder: all milestones/newest, wins less losses".into(), t.dim)];
        for i in islands {
            // Short: the card is half the window.
            let yard = i.yard.map(|(_, w)| format!(", yard {:.0}%", w * 100.0)).unwrap_or_default();
            let lad = i.ladder.map(|(_, a, top)| format!(", ladder {:+.2}/{:+.2}", a, top)).unwrap_or_default();
            let fail = if i.failures > 0 { format!(", {} failed", i.failures) } else { String::new() };
            rows.push((format!("{}: g{}{}{}{}", i.name.trim_end_matches("-cuda"), i.generations, yard, lad, fail), if i.failures >= 3 { t.alarm } else { t.ink }));
        }
        let best = islands.iter().map(|i| i.msteps).fold(0.0, f64::max);
        if best > 0.0 {
            rows.push((format!("fastest: {:.0} M tank-steps a second", best), t.ink));
        }
        lines(ui, at, y, &rows);
        return;
    }
    if !c.found {
        let y = card(ui, at, "crucible", (Light::Hollow, t.warn, "not found"));
        lines(ui, at, y, &[(format!("expected at {} (or ORRERY_CRUCIBLE)", c.dir), t.dim)]);
        return;
    }
    let y = card(ui, at, "crucible", (Light::Hollow, t.warn, "built; not in the queue yet"));
    let mut rows: Vec<(String, crate::draw::Rgb)> = vec![("a self-learning loop entirely on the GPU".into(), t.dim)];
    let best = c.runs.iter().map(|r| r.msteps).fold(0.0, f64::max);
    for r in c.runs.iter().rev().take(3) {
        let yard = r.yard.map(|(g, w)| format!(", yardstick {:.0}% at {}", w * 100.0, g)).unwrap_or_default();
        rows.push((format!("{}: {} generations{}", r.name, r.gens, yard), t.ink));
    }
    if best > 0.0 {
        rows.push((format!("fastest: {:.0} M tank-steps a second", best), t.ink));
    }
    let cards: Vec<String> = m.machines.iter().filter_map(|x| x.gpu.as_ref().map(|g| format!("{} ({})", x.name, short_gpu(&g.name)))).collect();
    rows.push((format!("for the GPUs: {}", if cards.is_empty() { "none answering".to_string() } else { cards.join(", ") }), t.dim));
    lines(ui, at, y, &rows);
}

/// "NVIDIA GeForce GTX 1080 Ti" -> "1080 Ti".
fn short_gpu(name: &str) -> String {
    name.rsplit("GTX ").next().or_else(|| name.rsplit("RTX ").next()).unwrap_or(name).trim().to_string()
}

/// A row per machine: what it has, and each project's work on it.
/// Each project's own buttons, on every machine at once: the band over the
/// matrix. Pause lets what runs finish; Stop now sends it back, and asks twice.
fn draw_project_buttons(ui: &mut Ui, at: Rect, q: &crate::training::Queue, st: &mut State) -> i32 {
    let t = ui.t;
    if !st.operator {
        return at.y;
    }
    let mut x = at.x;
    for (name, paused) in &q.projects {
        ui.label(x, at.y + 5, name, &F8X13, if *paused { t.warn } else { t.ink });
        x += F8X13.measure("treadcore ") as i32 + 6;
        let (word, how) = if *paused { ("Resume", "resume") } else { ("Pause", "pause") };
        if ui.button(rect(x, at.y, 70, 22), word, true) {
            st.ctl = Some(format!("project {} {}", name, how));
        }
        x += 76;
        let running = q.roster.iter().any(|r| !r.jobs.is_empty());
        if running {
            let key = format!("project:{}", name);
            let armed = st.armed_machine.as_deref() == Some(key.as_str());
            let w = if armed { 150 } else { 80 };
            if ui.button(rect(x, at.y, w, 22), if armed { "Press: jobs go back" } else { "Stop now" }, true) {
                if armed {
                    st.ctl = Some(format!("project {} stop", name));
                    st.armed_machine = None;
                } else {
                    st.armed_machine = Some(key);
                }
            }
            x += w + 6;
        }
        x += 24;
    }
    at.y + 30
}

/// Which machine runs which, and, for an operator, the switch: a press in a
/// project's column takes that machine off that project or puts it on.
fn draw_roster_matrix(ui: &mut Ui, at: Rect, m: &Model, q: &crate::training::Queue, st: &mut State) -> i32 {
    use crate::training::{display, machine_buttons, roster_lamp, Roster};
    let t = ui.t;
    let lh = F8X13.height as i32;
    let row_h = lh + 16;
    let x_name = at.x + 22;
    let x_hw = at.x + 136;
    let side = if st.operator { 320 } else { 0 };
    let x_tc = at.x + 300;
    let x_cr = x_tc + (at.w - 300 - side) / 2;
    ui.label(at.x, at.y, "Which machine runs which", &F8X13, t.accent);
    let hy = at.y + lh + 6;
    for (x, s) in [(x_name, "machine"), (x_hw, "CPU, GPU"), (x_tc, "treadcore"), (x_cr, "crucible")] {
        ui.label(x, hy, s, &F8X13, t.dim);
    }
    let mut y = hy + lh + 6;
    let mut rows: Vec<&Roster> = q.roster.iter().filter(|r| r.joined).collect();
    rows.extend(q.roster.iter().filter(|r| !r.joined));
    for r in rows {
        if y + row_h > at.bottom() {
            break;
        }
        let row = rect(at.x, y - 3, at.w, row_h);
        ui.panel(row, t.tile);
        ui.outline(row, t.line);
        let ty = row.y + (row_h - lh) / 2;
        let (c, lamp) = roster_lamp(ui, r);
        ui.light(at.x + 10, ty + lh / 2, 4, lamp, c);
        ui.label_in(x_name, ty, x_hw - x_name - 6, &display(&r.name), &F8X13, if r.joined { t.ink } else { t.dim });
        let mc = m.machines.iter().find(|x| x.name == r.name || (x.name == "this Mac" && r.name == "mac"));
        let hw = mc.map(|x| format!("{} cores{}", x.cores, x.gpu.as_ref().map(|g| format!(", {}", short_gpu(&g.name))).unwrap_or_default())).unwrap_or_default();
        ui.label_in(x_hw, ty, x_tc - x_hw - 6, &hw, &F8X13, t.dim);
        let lane_w = [x_cr - x_tc - 8, at.right() - side - x_cr - 8];
        for (i, (project, x)) in [("treadcore", x_tc), ("crucible", x_cr)].iter().enumerate() {
            let on = r.on.iter().find(|(p, _)| p == project).and_then(|(_, v)| *v);
            match on {
                Some(v) if r.joined => {
                    let job = r.jobs.iter().find(|(k, _, _)| (*project == "crucible") == (k == "crucible"));
                    let word = if v { "On" } else { "Off" };
                    if st.operator && ui.button(rect(*x, row.y + 3, 44, row_h - 6), word, true) {
                        st.ctl = Some(format!("assign {} {} {}", r.name, project, if v { "off" } else { "on" }));
                    }
                    let tx = *x + if st.operator { 50 } else { 0 };
                    let (what, col) = match job {
                        Some((k, title, _)) => (format!("{}: {}", k, title), t.up),
                        None if !st.operator => (word.to_string(), t.dim),
                        None => (String::new(), t.dim),
                    };
                    ui.label_in(tx, ty, lane_w[i] - (tx - *x), &what, &F8X13, col);
                }
                _ => {
                    ui.label(*x, ty, "-", &F8X13, t.line);
                }
            }
        }
        if st.operator {
            let (_, ask) = machine_buttons(ui, row.right() - 6, row.y + (row_h - 22) / 2, r, &mut st.armed_machine);
            if ask.is_some() {
                st.ctl = ask;
            }
        }
        y += row_h + 4;
    }
    y
}

fn draw_matrix(ui: &mut Ui, at: Rect, m: &Model, c: &Crucible, st: &mut State) -> i32 {
    let t = ui.t;
    if let Some(q) = m.queue.as_ref().filter(|q| !q.roster.is_empty()) {
        return draw_roster_matrix(ui, at, m, q, st);
    }
    let lh = F8X13.height as i32;
    let x_name = at.x + 10;
    let x_cpu = at.x + 150;
    let x_gpu = at.x + 260;
    let x_tc = at.x + 380;
    let x_cr = at.x + (at.w * 2 / 3).max(700.min(at.w - 260));
    ui.label(at.x, at.y, "Which machine runs which", &F8X13, t.accent);
    let hy = at.y + lh + 6;
    for (x, s) in [(x_name, "machine"), (x_cpu, "CPU"), (x_gpu, "GPU"), (x_tc, "treadcore"), (x_cr, "crucible")] {
        ui.label(x, hy, s, &F8X13, t.dim);
    }
    let mut y = hy + lh + 6;
    let lanes = m.queue.as_ref().map(|q| &q.lanes[..]).unwrap_or(&[]);
    for mc in &m.machines {
        if y + lh + 8 > at.bottom() {
            break;
        }
        let row = rect(at.x, y - 3, at.w, lh + 8);
        ui.panel(row, t.tile);
        ui.outline(row, t.line);
        ui.label_in(x_name, y + 1, x_cpu - x_name - 8, &mc.name, &F8X13, t.ink);
        ui.label(x_cpu, y + 1, &format!("{} cores", mc.cores), &F8X13, t.ink);
        let gpu = mc.gpu.as_ref().map(|g| short_gpu(&g.name)).unwrap_or_else(|| "-".to_string());
        ui.label(x_gpu, y + 1, &gpu, &F8X13, if mc.gpu.is_some() { t.ink } else { t.dim });
        // treadcore: the machine's lane in the queue.
        let lane = lanes.iter().find(|l| l.host == mc.name || (mc.name == "this Mac" && l.host == "mac"));
        let (lamp, col, word) = match lane {
            Some(l) if l.state == "running" => (Light::Full, t.up, format!("{}: {}", l.kind, l.title)),
            Some(l) => (Light::Hollow, t.dim, l.state.clone()),
            None => (Light::Hollow, t.dim, "no lane".to_string()),
        };
        ui.light(x_tc + 4, y + lh / 2 + 1, 4, lamp, col);
        ui.label_in(x_tc + 14, y + 1, x_cr - x_tc - 24, &word, &F8X13, col);
        // crucible: the machine's GPU lane, if it has one.
        let cl = lanes.iter().find(|l| l.host == format!("{}-cuda", mc.name));
        let isl = m.queue.as_ref().and_then(|q| q.islands.iter().find(|i| format!("{}-cuda", mc.name) == i.name || i.name == mc.name));
        let (lamp, col, word) = if let Some(l) = cl {
            let gens = isl.map(|i| format!(", {} generations", i.generations)).unwrap_or_default();
            if l.state == "running" {
                (Light::Full, t.up, format!("{}{}", l.title, gens))
            } else {
                (Light::Hollow, t.dim, format!("lane {}{}", l.state, gens))
            }
        } else if mc.gpu.is_none() {
            (Light::Hollow, t.line, "-".to_string())
        } else if c.found {
            (Light::Half, t.warn, "ready, not queued".to_string())
        } else {
            (Light::Hollow, t.dim, "crucible not found".to_string())
        };
        ui.light(x_cr + 4, y + lh / 2 + 1, 4, lamp, col);
        ui.label_in(x_cr + 14, y + 1, at.right() - x_cr - 20, &word, &F8X13, col);
        y += lh + 12;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_file_gives_its_length_its_last_yardstick_and_its_speed() {
        let tsv = "gen\tbest\tmean\tplay_s\tsteps_per_s\tnet_gflops\thall\tyard_win\tyard_draw\tyard_loss\n\
                   0\t3.9\t0.1\t0.6\t236700000\t1015\t1\t-1.0000\t-1.0000\t-1.0000\n\
                   9\t5.0\t1.1\t0.6\t266100000\t1141\t3\t0.0100\t0.0200\t0.9700\n\
                   10\t5.0\t1.1\t0.6\t250000000\t1100\t3\t-1.0000\t-1.0000\t-1.0000\n";
        let r = parse_run("whitewolf-spar", tsv);
        assert_eq!(r.gens, 11);
        assert_eq!(r.yard, Some((10, 0.01)));
        assert!((r.msteps - 266.1).abs() < 0.01);
    }

    #[test]
    fn a_card_never_says_a_shut_down_queue_is_adding_work_or_lists_a_paused_machine() {
        let doc = r#"{"queue":{"name":"grove","state":"shut down","adding":true,"counts":{},"lanes":[],
            "projects":[{"name":"treadcore","paused":false}],
            "roster":[{"name":"a","state":"no keeper","joined":true,"paused":false,"jobs":[],"on":{"treadcore":true}},
                      {"name":"b","state":"paused","joined":true,"paused":true,"jobs":[],"on":{"treadcore":true}}]}}"#;
        let m = Model::parse(doc, "");
        let q = m.queue.as_ref().unwrap();
        assert!(!q.keeper_up());
        assert_eq!(q.roster.len(), 2);
        assert_eq!(q.roster[0].on, vec![("treadcore".to_string(), Some(true))]);
        assert!(q.roster[1].paused);
    }

    #[test]
    fn a_card_is_named_short() {
        assert_eq!(short_gpu("NVIDIA GeForce GTX 1080 Ti"), "1080 Ti");
        assert_eq!(short_gpu("NVIDIA GeForce GTX 1070 Ti"), "1070 Ti");
    }
}
