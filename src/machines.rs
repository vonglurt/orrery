//! The Machines view: a fresh install to a fleet node.
//!
//! After the Media view writes a stick, and a person installs Ubuntu 24.04
//! with OpenSSH and their key (ssh-copy-id), a machine is added here by its
//! address. orrery reads it (Probe), applies orrery's own setup scripts to it
//! (the recipes), and joins it to the fleet. `tools/machines.py` does the
//! work; this draws its document and asks it for each step.
//!
//! A RECIPE IS HARD TO PRESS. Each needs root, and running the wrong one on
//! the wrong machine is the mistake to prevent (Paul, 28 October: "Make them
//! difficult to press (with confirm with details), we dont wont to run the
//! wrong scripts"). Apply opens a screen with everything that will happen --
//! the machine and what it is, the files and their sha256, the command -- and
//! its button is lit only once the machine's name is typed. Root itself is
//! never taken here: the files are copied, and the command is typed into the
//! machine's Sessions tile, where the person types the sudo password.
//! Every recipe checks before it acts, so a second run is safe.

use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::ui::{rect, Field, Ui};

const HEADER_H: i32 = 32;
const PAD: i32 = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub state: String,
    pub why: String,
    pub command: String,
    pub does: Vec<String>,
    pub files: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub target: String,
    pub joined: bool,
    pub probed: String,
    pub error: String,
    pub facts: Vec<(String, String)>,
    pub recipes: Vec<Recipe>,
    pub log: Vec<String>,
}

impl Machine {
    pub fn fact(&self, k: &str) -> &str {
        self.facts.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub fleet: String,
    pub machines: Vec<Machine>,
    pub why: String,
}

fn s(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

impl Model {
    pub fn parse(text: &str) -> Model {
        let v = match json::parse(text) {
            Ok(v) => v,
            Err(_) => return Model { why: if text.is_empty() { "reading the machines...".into() } else { text.lines().last().unwrap_or("").into() }, ..Model::default() },
        };
        let arr = |v: &json::Value, k: &str| v.get(k).and_then(|x| x.as_array()).map(|a| a.to_vec()).unwrap_or_default();
        let machines = arr(&v, "machines")
            .iter()
            .map(|m| Machine {
                name: s(m, "name"),
                target: s(m, "target"),
                joined: matches!(m.get("joined"), Some(json::Value::Bool(true))),
                probed: s(m, "probed"),
                error: s(m, "error"),
                facts: match m.get("facts") {
                    Some(json::Value::Obj(f)) => f.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect(),
                    _ => Vec::new(),
                },
                recipes: arr(m, "recipes")
                    .iter()
                    .map(|r| Recipe {
                        id: s(r, "id"),
                        title: s(r, "title"),
                        state: s(r, "state"),
                        why: s(r, "why"),
                        command: s(r, "command"),
                        does: arr(r, "does").iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect(),
                        files: arr(r, "files").iter().map(|f| (s(f, "name"), s(f, "sha256"))).collect(),
                    })
                    .collect(),
                log: arr(m, "log")
                    .iter()
                    .map(|l| {
                        let what = if l.get("staged").is_some() {
                            format!("staged {}: {}", s(l, "staged"), s(l, "command"))
                        } else if l.get("joined").is_some() {
                            "joined the fleet".to_string()
                        } else {
                            String::new()
                        };
                        format!("{} {}", s(l, "t").replace('T', " "), what)
                    })
                    .collect(),
            })
            .collect();
        Model { fleet: s(&v, "fleet"), machines, why: String::new() }
    }
}

/// What the person has asked for, for the loop to run (machines.py).
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Probe(String),
    Add(String, String),
    /// Copy the recipe's files, then type its command in the machine's tile.
    Apply { recipe: String, machine: String, target: String, command: String },
    Join(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Confirm {
    Apply(String),
    Join,
}

#[derive(Default)]
pub struct State {
    pub sel: String,
    pub operator: bool,
    confirm: Option<(Confirm, String)>,
    typed: Field,
    add: Field,
    /// What machines.py last said.
    pub said: String,
    pub ask: Option<Ask>,
}

impl State {
    pub fn new(operator: bool) -> State {
        State { operator, ..State::default() }
    }

    /// Join's confirmation, for the selected machine (the menu's Join...).
    pub fn confirm_join(&mut self) {
        if !self.sel.is_empty() {
            self.confirm = Some((Confirm::Join, self.sel.clone()));
            self.typed.clear();
        }
    }

    /// The confirmation, open: for a review frame of it (ORRERY_FRAME_CONFIRM),
    /// and the menu's Apply items.
    pub fn confirm_apply(&mut self, recipe: &str, machine: &str) {
        self.sel = machine.to_string();
        self.confirm = Some((Confirm::Apply(recipe.to_string()), machine.to_string()));
        self.typed.clear();
    }
}

pub fn draw(ui: &mut Ui, m: &Model, st: &mut State) -> Option<nav::View> {
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let t = ui.t;
    ui.panel(rect(0, 0, w, h), t.base);
    let (header, rest) = rect(0, 0, w, h).cut_top(HEADER_H);
    ui.panel(header, t.panel);
    ui.hrule(header.x, header.bottom() - 1, header.w);
    let ty = header.y + (HEADER_H - F10X20.height as i32) / 2;
    ui.label(PAD, ty, "orrery", &F10X20, t.ink);
    let nav_at = rect(PAD + F10X20.measure("orrery") as i32 + 14, header.y, nav::width(), HEADER_H);
    let mut go = nav::strip(ui, nav_at, nav::View::Machines);
    if st.confirm.is_none() {
        if let Some(v) = nav::chord(ui) {
            go = Some(v);
        }
    }
    let sy = header.y + (HEADER_H - F8X13.height as i32) / 2;
    nav::header_right(ui, header, PAD, sy, "a fresh install to a fleet node: probe, apply, join");

    let body = rest.inset(PAD);
    if !m.why.is_empty() {
        ui.label_in(body.x, body.y + 8, body.w, &m.why, &F8X13, t.dim);
        return go;
    }
    if st.sel.is_empty() || !m.machines.iter().any(|x| x.name == st.sel) {
        st.sel = m.machines.first().map(|x| x.name.clone()).unwrap_or_default();
    }
    let lh = F8X13.height as i32 + 6;

    // ---- the machines --------------------------------------------------------
    let lw = 300.min(body.w / 3);
    let mut y = body.y;
    ui.label(body.x, y, "Machines", &F8X13, t.ink);
    y += lh;
    for x in &m.machines {
        let r = rect(body.x, y - 2, lw, lh * 2 - 2);
        let on = x.name == st.sel;
        ui.panel(r, if on { t.tile } else { t.base });
        if on {
            ui.outline(r, t.accent);
        }
        let open = x.recipes.iter().filter(|r| r.state == "not applied").count();
        let tag = if x.target == "-" { format!("this Mac: {}", x.fact("hostname")) } else if x.joined { "in the fleet".into() } else { "new".into() };
        ui.label_in(r.x + 6, y, lw - 12, &format!("{}  {}", x.name, tag), &F8X13, if x.joined { t.ink } else { t.up });
        let sys = if !x.error.is_empty() {
            x.error.clone()
        } else {
            let gpu = x.fact("gpu");
            let gpu = gpu.split('[').nth(1).map(|g| g.split(']').next().unwrap_or("")).unwrap_or("");
            format!("{}{}{}", x.fact("pretty"), if gpu.is_empty() { "" } else { " · " }, gpu)
        };
        ui.label_in(r.x + 6, y + lh - 3, lw - 12, &format!("{}{}", sys, if open > 0 { format!(" · {} to apply", open) } else { String::new() }), &F8X13, if x.error.is_empty() { t.dim } else { t.warn });
        if st.confirm.is_none() && ui.button_hit(r) {
            st.sel = x.name.clone();
        }
        y += lh * 2;
    }
    // A new machine: a fresh Ubuntu 24.04 with OpenSSH and the key copied.
    if st.operator {
        y += 8;
        ui.label_in(body.x, y, lw, "Add a machine: NAME USER@ADDRESS", &F8X13, t.dim);
        y += lh;
        let fr = rect(body.x, y - 3, lw - 60, lh);
        ui.field(fr, &st.add, "name user@address");
        let entered = if st.confirm.is_none() { st.add.feed(&ui.f) } else { false };
        let parts: Vec<String> = st.add.text.split_whitespace().map(|x| x.to_string()).collect();
        let ok = parts.len() == 2 && parts[1].contains('@');
        if (ui.button(rect(body.x + lw - 54, y - 3, 54, lh), "Add", ok) || entered) && ok && st.confirm.is_none() {
            st.ask = Some(Ask::Add(parts[0].clone(), parts[1].clone()));
            st.add.clear();
        }
        y += lh + 6;
        ui.label_in(body.x, y, lw, "Before: install Ubuntu 24.04 with OpenSSH (Media), then ssh-copy-id USER@ADDRESS", &F8X13, t.dim);
    }

    // ---- the chosen machine ---------------------------------------------------------
    let Some(x) = m.machines.iter().find(|x| x.name == st.sel) else { return go };
    let rx = body.x + lw + 20;
    let rw = body.right() - rx;
    let mut y = body.y;
    let head = if x.target == "-" { format!("{}  this Mac ({}), the coordinator: it runs the keeper and pushes", x.name, x.fact("hostname")) } else { format!("{}  {}", x.name, x.target) };
    ui.label_in(rx, y, rw - 210, &head, &F8X13, t.ink);
    if st.operator && st.confirm.is_none() && ui.button(rect(body.right() - 90, y - 4, 90, 22), "Probe", true) {
        st.ask = Some(Ask::Probe(x.name.clone()));
    }
    if st.operator && !x.joined && st.confirm.is_none() && ui.button(rect(body.right() - 196, y - 4, 100, 22), "Join...", true) {
        st.confirm = Some((Confirm::Join, x.name.clone()));
        st.typed.clear();
    }
    y += lh + 2;
    let facts: [(&str, String); 7] = [
        ("system", format!("{} ({} {})", x.fact("pretty"), x.fact("kernel"), x.fact("arch"))),
        ("CPU", format!("{} · {} threads · {} GB", x.fact("cpu_model"), x.fact("cpus"), x.fact("mem_gb"))),
        ("GPU", if x.fact("os_id") == "macos" {
            format!("{} (Metal; no CUDA)", or(x.fact("gpu"), "?"))
        } else if x.fact("gpu").is_empty() && x.fact("nvidia_pci").is_empty() {
            "none".into()
        } else {
            format!("{} · driver {} · CUDA {}", x.fact("gpu"), or(x.fact("nvidia_driver"), "none"), or(x.fact("cuda"), "none"))
        }),
        ("disk, net", format!("{} on / · {}", x.fact("disk"), x.fact("addrs").trim_end_matches(';').replace(';', ", "))),
        ("fleet tools", format!("Rust {} · treadcore {} · tmux {} · copal-tm {}", or(x.fact("cargo").split_whitespace().nth(1).unwrap_or(""), "no"), or(x.fact("treadcore"), "no"), yes(x.fact("tmux")), yes(x.fact("copal_tm")))),
        ("login, power", if x.fact("os_id") == "macos" {
            format!("Orrery.app {} · Copal TM.app {} · up {}", yes(x.fact("orrery_app")), yes(x.fact("copal_tm_app")), x.fact("uptime"))
        } else {
            format!("topnotch {} · power-off {}", yes(x.fact("topnotch")), or(x.fact("poweroff"), "no"))
        }),
        ("read", if x.probed.is_empty() { "not yet: Probe".into() } else { x.probed.replace('T', " ") }),
    ];
    for (k, v) in facts.iter() {
        ui.label(rx, y, k, &F8X13, t.dim);
        ui.label_in(rx + 110, y, rw - 110, v, &F8X13, t.ink);
        y += lh - 2;
    }
    y += 10;
    ui.label(rx, y, "Recipes (orrery's setup scripts; each needs root, each is safe to run again)", &F8X13, t.ink);
    y += lh;
    for r in &x.recipes {
        let c = match r.state.as_str() {
            "applied" => t.up,
            "not applied" => t.warn,
            _ => t.dim,
        };
        ui.label_in(rx, y, 280, &r.title, &F8X13, t.ink);
        let stx = if r.why.is_empty() { r.state.clone() } else { format!("{}: {}", r.state, r.why) };
        ui.label_in(rx + 290, y, rw - 290 - 110, &stx, &F8X13, c);
        let can = st.operator && r.state != "not for it";
        if can && st.confirm.is_none() && ui.button(rect(body.right() - 100, y - 3, 100, lh - 2), "Apply...", true) {
            st.confirm = Some((Confirm::Apply(r.id.clone()), x.name.clone()));
            st.typed.clear();
        }
        y += lh;
    }
    if !x.log.is_empty() {
        y += 8;
        ui.label(rx, y, "Done here", &F8X13, t.ink);
        y += lh;
        for l in x.log.iter().rev() {
            ui.label_in(rx, y, rw, l, &F8X13, t.dim);
            y += lh - 2;
        }
    }
    if !st.said.is_empty() {
        ui.label_in(rx, body.bottom() - lh, rw, &st.said, &F8X13, t.dim);
    }

    // ---- the confirmation ----------------------------------------------------------
    if let Some((what, name)) = st.confirm.clone() {
        let pane = rect(body.x + body.w / 8, body.y + 20, body.w * 3 / 4, body.h - 40);
        ui.panel(pane, t.tile);
        ui.outline(pane, t.alarm);
        let ix = pane.x + 16;
        let iw = pane.w - 32;
        let mut y = pane.y + 14;
        let mut lines: Vec<(String, bool)> = Vec::new();
        let title;
        match &what {
            Confirm::Apply(rid) => {
                let r = x.recipes.iter().find(|r| &r.id == rid).cloned().unwrap_or_default();
                title = format!("Apply \"{}\" to {}?", r.title, name);
                lines.push((format!("machine   {}  ({})", name, x.target), false));
                lines.push((format!("system    {}", x.fact("pretty")), false));
                lines.push((format!("GPU       {}", or(x.fact("gpu"), "none")), false));
                lines.push((format!("now       {}", r.state), false));
                lines.push(("".into(), false));
                lines.push(("It will:".into(), true));
                for d in &r.does {
                    lines.push((format!("  - {}", d), false));
                }
                lines.push(("".into(), false));
                lines.push(("Copied to ~/orrery-recipes/ on the machine, checked there by sha256:".into(), true));
                for (f, sum) in &r.files {
                    lines.push((format!("  {:<26} {}", f, &sum[..sum.len().min(32)]), false));
                }
                lines.push(("".into(), false));
                lines.push((format!("Then typed into {}'s Sessions tile (a new window, 'recipe'), where you type the password:", name), true));
                lines.push((format!("  {}", r.command), false));
            }
            Confirm::Join => {
                title = format!("Join {} to the fleet?", name);
                let gpu = !x.fact("nvidia_smi").is_empty() && !x.fact("cuda").is_empty();
                lines.push((format!("machine   {}  ({})", name, x.target), false));
                lines.push((format!("system    {}", x.fact("pretty")), false));
                lines.push(("".into(), false));
                lines.push(("It will:".into(), true));
                lines.push((format!("  - add it to the queue's machines, caps {}: its lane starts, builds the newest commit, and takes jobs", if gpu { "cpu and gpu" } else { "cpu" }), false));
                lines.push(("  - add it to the fleet's nodes list (the Sessions grid, copal fleet)".into(), false));
                if !x.fact("poweroff").is_empty() {
                    lines.push(("  - let a fleet shutdown power it off (its power-off rule is there)".into(), false));
                }
            }
        }
        ui.label_in(ix, y, iw, &title, &F10X20, t.ink);
        y += F10X20.height as i32 + 10;
        for (l, bold) in &lines {
            ui.label_in(ix, y, iw, l, &F8X13, if *bold { t.ink } else { t.dim });
            y += lh - 4;
        }
        y += 10;
        ui.label_in(ix, y, iw, &format!("Type {} to confirm:", name), &F8X13, t.alarm);
        y += lh;
        let fr = rect(ix, y, 260, lh + 2);
        ui.field(fr, &st.typed, "");
        let entered = st.typed.feed(&ui.f);
        let ok = st.typed.text == name;
        let label = match &what {
            Confirm::Apply(_) => "Apply",
            Confirm::Join => "Join",
        };
        if (ui.button(rect(ix + 280, y, 100, lh + 2), label, ok) || (entered && ok)) && ok {
            st.ask = Some(match &what {
                Confirm::Apply(rid) => {
                    let r = x.recipes.iter().find(|r| &r.id == rid).cloned().unwrap_or_default();
                    Ask::Apply { recipe: rid.clone(), machine: name.clone(), target: x.target.clone(), command: r.command }
                }
                Confirm::Join => Ask::Join(name.clone()),
            });
            st.confirm = None;
        }
        if ui.button(rect(ix + 390, y, 100, lh + 2), "Cancel", true) {
            st.confirm = None;
        }
    }
    go
}

fn or<'a>(v: &'a str, d: &'a str) -> &'a str {
    if v.is_empty() {
        d
    } else {
        v
    }
}

fn yes(v: &str) -> &'static str {
    if v.is_empty() {
        "no"
    } else {
        "yes"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_document_reads_into_machines_and_recipes() {
        let doc = r#"{"fleet": "grove", "machines": [{"name": "whitewolf", "target": "user@192.168.8.124", "joined": false,
            "probed": "2026-10-28T15:00:00", "error": "", "facts": {"pretty": "Ubuntu 24.04.5 LTS", "gpu": "NVIDIA [GeForce GTX 1080]"},
            "recipes": [{"id": "fleet-node", "title": "Make it a fleet node", "state": "not applied", "why": "",
            "command": "cd ~/orrery-recipes && sudo bash fleet-node.sh", "does": ["the build tools"],
            "files": [{"name": "fleet-node.sh", "sha256": "ab"}]}], "log": [{"t": "2026-10-28T15:01:00", "joined": {}}]}]}"#;
        let m = Model::parse(doc);
        assert_eq!(m.fleet, "grove");
        let x = &m.machines[0];
        assert_eq!(x.name, "whitewolf");
        assert!(!x.joined);
        assert_eq!(x.fact("pretty"), "Ubuntu 24.04.5 LTS");
        assert_eq!(x.recipes[0].command, "cd ~/orrery-recipes && sudo bash fleet-node.sh");
        assert_eq!(x.recipes[0].files, vec![("fleet-node.sh".to_string(), "ab".to_string())]);
        assert_eq!(x.log, vec!["2026-10-28 15:01:00 joined the fleet".to_string()]);
    }

    #[test]
    fn a_failed_read_says_why_rather_than_drawing_nothing() {
        assert_eq!(Model::parse("").why, "reading the machines...");
        assert_eq!(Model::parse("cannot reach it\nno such queue").why, "no such queue");
    }
}
