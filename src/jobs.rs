//! The jobs: what the fleet is working on, and the commands doing it.
//!
//! The room of machines shows what each machine IS. This shows what the
//! fleet is DOING when it is used as one computer: a loop of rounds
//! (`tools/fleet-train.sh`), each round's stages, and for the round in hand
//! every machine's lane -- the job it holds, how long it has held it, and the
//! command pipe that is doing the work, spelled as a person would type it.
//!
//! THE SAME RULE AS EVERY OTHER FACE. Nothing here is discovered. The whole
//! picture is the document one command prints (`--jobs-cmd`, by default
//! nothing: the view then says how to give it one), polled on a thread like
//! `copal fleet state`. A job that is not in that document cannot be drawn,
//! and this face has no verbs: watching a cluster and steering it are two
//! things, and the second is still the scripts' (docs/fleet-train.md).

use crate::draw::{mix, Rgb};
use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::ui::{rect, Light, Rect, Ui};

const HEADER_H: i32 = 32;
const PAD: i32 = 10;
const LANE_H: i32 = 40;
const ROUND_H: i32 = 22;

/// One machine's lane in a recording run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lane {
    pub host: String,
    pub threads: i64,
    pub nice: i64,
    pub jobs: i64,
    pub seeds: i64,
    pub seeds_per_s: f64,
    pub state: String,
    pub job: String,
    pub age_s: Option<i64>,
    pub pipe: String,
}

/// A recording run: a queue of jobs and the lanes taking them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub name: String,
    pub queued: i64,
    pub running: i64,
    pub done: i64,
    pub failed: i64,
    pub total: i64,
    pub seeds: i64,
    pub bytes: f64,
    pub ended: bool,
    pub lanes: Vec<Lane>,
}

/// A round of the loop and its stages, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Round {
    pub n: i64,
    pub state: String,
    /// (stage, state, detail), in the loop's stage order.
    pub stages: Vec<(String, String, String)>,
    pub summary: String,
}

/// The document, read. `why` is set when there is nothing to draw, and says
/// why in a sentence.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub why: String,
    pub source: String,
    pub best: String,
    pub stopping: bool,
    pub rounds: Vec<Round>,
    pub runs: Vec<Run>,
}

fn s_of(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n_of(v: &json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0) as i64
}

impl Model {
    pub fn parse(text: &str, source: &str) -> Model {
        let mut m = Model { source: source.to_string(), ..Model::default() };
        if text.trim().is_empty() {
            m.why = if source.is_empty() {
                "no jobs command: start orrery with --jobs-cmd \"tools/fleet-train.sh status STAGES --json\"".into()
            } else {
                format!("waiting for the first answer from: {}", source)
            };
            return m;
        }
        let v = match json::parse(text) {
            Ok(v) => v,
            Err(e) => {
                m.why = format!("the jobs command did not print JSON: {}", e.0);
                return m;
            }
        };
        if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
            m.why = format!("the jobs command failed: {}", e);
            return m;
        }
        m.best = v.get("best").and_then(|x| x.as_str()).unwrap_or("").to_string();
        m.stopping = v.get("stopping").and_then(|x| x.as_bool()).unwrap_or(false);
        let order: Vec<String> = v
            .get("stages")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        for r in v.get("rounds").and_then(|x| x.as_array()).unwrap_or(&[]) {
            let mut round = Round {
                n: n_of(r, "round"),
                state: s_of(r, "state"),
                summary: s_of(r, "summary"),
                stages: Vec::new(),
            };
            for name in &order {
                let st = r.get("stages").and_then(|s| s.get(name));
                round.stages.push((
                    name.clone(),
                    st.map(|s| s_of(s, "state")).unwrap_or_default(),
                    st.map(|s| s_of(s, "detail")).unwrap_or_default(),
                ));
            }
            m.rounds.push(round);
        }
        for r in v.get("runs").and_then(|x| x.as_array()).unwrap_or(&[]) {
            let mut run = Run {
                name: s_of(r, "run"),
                queued: n_of(r, "queued"),
                running: n_of(r, "running"),
                done: n_of(r, "done"),
                failed: n_of(r, "failed"),
                total: n_of(r, "total"),
                seeds: n_of(r, "seeds"),
                bytes: r.get("bytes").and_then(|x| x.as_num()).unwrap_or(0.0),
                ended: r.get("ended").and_then(|x| x.as_bool()).unwrap_or(false),
                lanes: Vec::new(),
            };
            for l in r.get("lanes").and_then(|x| x.as_array()).unwrap_or(&[]) {
                run.lanes.push(Lane {
                    host: s_of(l, "host"),
                    threads: n_of(l, "threads"),
                    nice: n_of(l, "nice"),
                    jobs: n_of(l, "jobs"),
                    seeds: n_of(l, "seeds"),
                    seeds_per_s: l.get("seeds_per_s").and_then(|x| x.as_num()).unwrap_or(0.0),
                    state: s_of(l, "state"),
                    job: s_of(l, "job"),
                    age_s: l.get("age_s").and_then(|x| x.as_num()).map(|a| a as i64),
                    pipe: s_of(l, "pipe"),
                });
            }
            m.runs.push(run);
        }
        if m.rounds.is_empty() && m.runs.is_empty() {
            m.why = "the jobs command answered, and there are no rounds or runs yet".into();
        }
        m
    }
}

/// A duration as a person reads one: 40 s, 12 min, 2 h 5 min.
fn ago(s: i64) -> String {
    if s < 90 {
        format!("{} s", s)
    } else if s < 5400 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

fn state_colour(ui: &Ui, state: &str) -> (Rgb, Light) {
    let t = ui.t;
    match state {
        "recording" | "started" | "running" => (t.up, Light::Full),
        "sending" => (t.accent, Light::Full),
        "done" | "finished" => (t.dim, Light::Hollow),
        "failed" => (t.alarm, Light::Full),
        _ => (t.dim, Light::Hollow),
    }
}

pub fn draw(ui: &mut Ui, m: &Model) -> Option<nav::View> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let t = ui.t;
    ui.panel(rect(0, 0, w, h), t.base);
    let (header, rest) = rect(0, 0, w, h).cut_top(HEADER_H);

    // ---- header, with the navigation strip -----------------------------
    ui.panel(header, t.panel);
    ui.hrule(header.x, header.bottom() - 1, header.w);
    let ty = header.y + (HEADER_H - F10X20.height as i32) / 2;
    ui.label(PAD, ty, "orrery", &F10X20, t.ink);
    let nav_at = rect(PAD + F10X20.measure("orrery") as i32 + 14, header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Jobs);
    if let Some(v) = nav::chord(ui) {
        go = Some(v);
    }
    let sy = header.y + (HEADER_H - F8X13.height as i32) / 2;
    let busy: i64 = m.runs.iter().map(|r| r.running).sum();
    let lanes: usize = m.runs.iter().map(|r| r.lanes.len()).sum();
    if m.why.is_empty() {
        let right = format!("{} machines  {} jobs running", lanes, busy);
        ui.label_right(header.right() - PAD, sy, &right, &F8X13, t.dim);
    }

    let body = rest.inset(PAD);
    if !m.why.is_empty() {
        ui.label_in(body.x, body.y + 8, body.w, &m.why, &F8X13, t.warn);
        return go;
    }

    let mut y = body.y;
    // ---- the loop -------------------------------------------------------
    if !m.rounds.is_empty() || !m.best.is_empty() {
        ui.label(body.x, y, "fleet-train", &F10X20, t.ink);
        let mut x = body.x + F10X20.measure("fleet-train") as i32 + 14;
        let line = if m.best.is_empty() { "no best yet".to_string() } else { format!("best: {}", m.best) };
        x += ui.label_in(x, y + 4, body.right() - x, &line, &F8X13, t.dim);
        if m.stopping {
            ui.label(x + 12, y + 4, "stopping after this round", &F8X13, t.warn);
        }
        y += F10X20.height as i32 + 6;
        // The newest rounds that fit, newest first: the round in hand is the
        // one being watched.
        let room = ((body.bottom() - y) / 3).max(ROUND_H * 2);
        let fit = (room / ROUND_H).max(1) as usize;
        for r in m.rounds.iter().rev().take(fit) {
            draw_round(ui, rect(body.x, y, body.w, ROUND_H), r);
            y += ROUND_H;
        }
        y += 10;
    }

    // ---- the runs: a queue each, a lane a machine -------------------------
    for run in &m.runs {
        if y + 40 > body.bottom() {
            break;
        }
        y = draw_run(ui, rect(body.x, y, body.w, body.bottom() - y), run) + 10;
    }
    go
}

fn draw_round(ui: &mut Ui, at: Rect, r: &Round) {
    let t = ui.t;
    let cy = at.y + (at.h - F8X13.height as i32) / 2;
    ui.label(at.x, cy, &format!("round {:>3}", r.n), &F8X13, t.ink);
    let mut x = at.x + F8X13.measure("round 000") as i32 + 12;
    // A chip a stage: lit while it runs, quiet when done, alarm when failed,
    // an empty outline while it has not been reached.
    for (name, state, _) in &r.stages {
        let cw = F8X13.measure(name) as i32 + 18;
        let chip = rect(x, at.y + 3, cw, at.h - 6);
        let (c, _) = state_colour(ui, state);
        match state.as_str() {
            "" => ui.outline(chip, t.line),
            "done" => {
                ui.panel(chip, t.tile);
                ui.outline(chip, t.line);
            }
            _ => ui.panel(chip, mix(t.tile, c, 90)),
        }
        let fg = if state.is_empty() { t.dim } else { t.ink };
        ui.label(chip.x + 9, cy, name, &F8X13, fg);
        x += cw + 6;
    }
    let summary = if !r.summary.is_empty() { r.summary.clone() } else { r.state.clone() };
    ui.label_in(x + 8, cy, at.right() - x - 8, &summary, &F8X13, if r.state == "failed" { t.alarm } else { t.dim });
}

/// One run; returns the y below it.
fn draw_run(ui: &mut Ui, at: Rect, run: &Run) -> i32 {
    let t = ui.t;
    let mut y = at.y;
    ui.label(at.x, y, &run.name, &F10X20, t.ink);
    let x = at.x + F10X20.measure(&run.name) as i32 + 14;
    let line = format!(
        "{} of {} jobs done  {} running  {} queued{}  {} seeds  {:.1} GB{}",
        run.done,
        run.total,
        run.running,
        run.queued,
        if run.failed > 0 { format!("  {} FAILED", run.failed) } else { String::new() },
        run.seeds,
        run.bytes / 1e9,
        if run.ended { "  ended" } else { "" }
    );
    ui.label_in(x, y + 4, at.right() - x, &line, &F8X13, if run.failed > 0 { t.alarm } else { t.dim });
    y += F10X20.height as i32 + 4;

    // The queue as one bar: done, running, queued.
    let bar = rect(at.x, y, at.w, 8);
    ui.panel(bar, t.tile);
    ui.outline(bar, t.line);
    if run.total > 0 {
        let dw = (bar.w as i64 * run.done / run.total) as i32;
        let rw = (bar.w as i64 * run.running / run.total) as i32;
        ui.c.rect(bar.x, bar.y, dw, bar.h, t.up);
        ui.c.rect(bar.x + dw, bar.y, rw, bar.h, t.accent);
    }
    y += 14;

    let col_host = at.x + 18;
    let col_rate = col_host + 120;
    let col_now = col_rate + F8X13.measure("00 jobs  0.00 seeds/s  8x nice 19") as i32 + 16;
    for l in &run.lanes {
        if y + LANE_H > at.bottom() {
            break;
        }
        let row = rect(at.x, y, at.w, LANE_H - 4);
        ui.panel(row, t.tile);
        let ty = row.y + 4;
        let (c, light) = state_colour(ui, &l.state);
        ui.light(at.x + 9, ty + F8X13.height as i32 / 2, 4, light, c);
        ui.label_in(col_host, ty, col_rate - col_host - 8, &l.host, &F8X13, t.ink);
        let rate = format!("{} jobs  {:.2} seeds/s  {}x nice {}", l.jobs, l.seeds_per_s, l.threads, l.nice);
        ui.label_in(col_rate, ty, col_now - col_rate - 8, &rate, &F8X13, t.dim);
        let now = match (&l.job[..], l.age_s) {
            ("", Some(a)) => format!("{}, {} ago", l.state, ago(a)),
            ("", None) => l.state.clone(),
            (j, Some(a)) => format!("{} {}, {}", l.state, j, ago(a)),
            (j, None) => format!("{} {}", l.state, j),
        };
        ui.label_in(col_now, ty, row.right() - col_now - 6, &now, &F8X13, c);
        // The pipe, in full where it fits: the command this lane is running.
        if !l.pipe.is_empty() {
            ui.label_in(col_host, ty + F8X13.height as i32 + 4, row.right() - col_host - 6, &format!("$ {}", l.pipe), &F8X13, t.dim);
        }
        y += LANE_H;
    }
    y
}

/// A document of the right shape for `--demo` and for review frames: a loop
/// in its second round, recording on four machines. Made up, and the view
/// says nothing to suggest otherwise only because `--demo` already does.
pub fn demo_doc() -> String {
    r#"{"stages":["record","train","judge"],"stopping":false,
"best":"bc2 (LR 108): census -19.76 +/- 0.39 against the defaults",
"rounds":[
 {"round":1,"state":"done","summary":"round 1: ft001 not better than the best (+0.21 +/- 0.38); the best stays bc2",
  "stages":{"record":{"state":"done","detail":"1491 s"},"train":{"state":"done","detail":"212 s"},"judge":{"state":"done","detail":"640 s"}}},
 {"round":2,"state":"running","stages":{"record":{"state":"started","detail":""}}}],
"runs":[{"run":"ft002","queued":61,"running":4,"done":35,"failed":0,"total":100,"seeds":420,"bytes":10215000000,"ended":false,
 "lanes":[
  {"host":"calber","threads":8,"nice":10,"jobs":14,"seeds":168,"seeds_per_s":0.81,"state":"recording","job":"9004032","age_s":12,
   "pipe":"ssh 192.168.8.181 \"RECORD=~/fleet/ft002/out/9004032.bin SEED0=9004032 THREADS=8 nice -n 10 census-bin --ignored learn::harness::train | tail -4\""},
  {"host":"mac","threads":5,"nice":19,"jobs":13,"seeds":156,"seeds_per_s":0.83,"state":"sending","job":"9004020","age_s":4,
   "pipe":"cat ~/fleet/ft002/out/9004020.bin | ssh 192.168.8.181 \"cat > ~/fleet/ft002/shards/9004020.bin.part\""},
  {"host":"columbia","threads":8,"nice":10,"jobs":5,"seeds":60,"seeds_per_s":0.18,"state":"recording","job":"9003996","age_s":71,
   "pipe":"ssh 192.168.8.224 \"RECORD=~/fleet/ft002/out/9003996.bin SEED0=9003996 THREADS=8 nice -n 10 census-bin --ignored learn::harness::train | tail -4\""},
  {"host":"nasser","threads":4,"nice":19,"jobs":3,"seeds":36,"seeds_per_s":0.09,"state":"recording","job":"9004008","age_s":102,
   "pipe":"ssh 192.168.8.121 \"RECORD=~/fleet/ft002/out/9004008.bin SEED0=9004008 THREADS=4 nice -n 19 census-bin --ignored learn::harness::train | tail -4\""}]}]}"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_document_reads_whole() {
        let m = Model::parse(&demo_doc(), "demo");
        assert!(m.why.is_empty(), "{}", m.why);
        assert_eq!(m.rounds.len(), 2);
        assert_eq!(m.rounds[1].stages[0], ("record".into(), "started".into(), "".into()));
        assert_eq!(m.rounds[1].stages[2].1, "", "a stage not reached has no state");
        assert_eq!(m.runs[0].lanes.len(), 4);
        assert_eq!(m.runs[0].lanes[1].state, "sending");
        assert!(m.runs[0].lanes[1].pipe.starts_with("cat "));
    }

    #[test]
    fn no_command_and_bad_output_each_say_why() {
        assert!(Model::parse("", "").why.contains("--jobs-cmd"));
        assert!(Model::parse("not json", "x").why.contains("did not print JSON"));
        assert!(Model::parse("{\"error\":\"exit 1\"}", "x").why.contains("exit 1"));
        assert!(Model::parse("{\"stages\":[],\"rounds\":[],\"runs\":[]}", "x").why.contains("no rounds"));
    }

    #[test]
    fn durations_read_as_a_person_says_them() {
        assert_eq!(ago(40), "40 s");
        assert_eq!(ago(720), "12 min");
        assert_eq!(ago(7500), "2 h 5 min");
    }
}
