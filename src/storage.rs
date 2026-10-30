//! The Storage view: the fleet's disks and shares, as a patch bay.
//!
//! Paul, 29 October: "a settings program in orrery which helps deploy and
//! configure and mount shares on various fleet members. Mounting eachothers
//! data. This is like a patch bay to ensure and monitor and add
//! connnectivity of disk." `tools/storage.py` reads every machine (Survey,
//! as the user, no root) and writes the document drawn here: each machine's
//! links and their speed, its disks and what each is for, its pools and
//! mounts, the shares, and the disks that could be Ceph's.
//!
//! THE PATCH BAY IS A GRID: a row a share, a column a machine. A dot is a
//! mount that is there; a + mounts the share on that machine. Sharing and
//! mounting need root, so they are staged as Apply is (machines.rs): the
//! script copied, its command typed into the machine's Sessions tile, the
//! password typed there by the person. NOTHING HERE FORMATS A DISK. The
//! Ceph candidates are a list to choose from, ranked fastest first; a disk
//! becomes Ceph's by a person's hand (docs/storage-lab-report.md).

use crate::font::{F10X20, F8X13};
use crate::json;
use crate::nav;
use crate::ui::{rect, Field, Ui};

const PAD: i32 = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Disk {
    pub name: String,
    pub size_gib: f64,
    pub kind: String,
    pub model: String,
    pub role: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mount {
    pub source: String,
    pub at: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub os: String,
    pub addr: String,
    pub mem_gb: i64,
    pub mbps: i64,
    pub ceph: String,
    pub nics: Vec<(String, i64, String)>,
    pub disks: Vec<Disk>,
    pub zpools: Vec<String>,
    pub mounts: Vec<Mount>,
    pub surveyed: String,
    pub error: String,
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Share {
    pub server: String,
    pub addr: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub machines: Vec<Machine>,
    pub shares: Vec<Share>,
    pub candidates: Vec<(String, Disk, String)>,
    pub why: String,
}

fn s(v: &json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn n(v: &json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0) as i64
}

fn disk(d: &json::Value) -> Disk {
    Disk {
        name: s(d, "name"),
        size_gib: d.get("size_gib").and_then(|x| x.as_num()).unwrap_or(0.0),
        kind: s(d, "kind"),
        model: s(d, "model"),
        role: s(d, "role"),
        why: s(d, "why"),
    }
}

impl Model {
    pub fn parse(text: &str) -> Model {
        let v = match json::parse(text) {
            Ok(v) => v,
            Err(_) => return Model { why: if text.is_empty() { "reading the fleet's disks...".into() } else { text.lines().last().unwrap_or("").into() }, ..Model::default() },
        };
        let arr = |v: &json::Value, k: &str| v.get(k).and_then(|x| x.as_array()).map(|a| a.to_vec()).unwrap_or_default();
        let machines = arr(&v, "machines")
            .iter()
            .map(|m| Machine {
                name: s(m, "name"),
                os: s(m, "os"),
                addr: s(m, "addr"),
                mem_gb: n(m, "mem_gb"),
                mbps: n(m, "mbps"),
                ceph: s(m, "ceph"),
                nics: arr(m, "nics").iter().map(|x| (s(x, "name"), n(x, "mbps"), s(x, "state"))).collect(),
                disks: arr(m, "disks").iter().map(disk).collect(),
                zpools: arr(m, "zpools").iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect(),
                mounts: arr(m, "mounts").iter().map(|x| Mount { source: s(x, "source"), at: s(x, "at"), kind: s(x, "kind") }).collect(),
                surveyed: s(m, "surveyed"),
                error: s(m, "error"),
                log: arr(m, "log").iter().map(|l| format!("{} {}", s(l, "t").replace('T', " "), s(l, "staged"))).collect(),
            })
            .collect();
        let shares = arr(&v, "exports").iter().map(|e| Share { server: s(e, "server"), addr: s(e, "addr"), path: s(e, "path") }).collect();
        let candidates = arr(&v, "candidates").iter().map(|c| (s(c, "machine"), disk(c), format!("{} · {} GB", speed(n(c, "mbps")), n(c, "mem_gb")))).collect();
        Model { machines, shares, candidates, why: String::new() }
    }
}

/// What the view asks storage.py for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Read every machine again (no root).
    Survey,
    /// Stage share-nfs.sh on the machine for this path.
    Share { machine: String, path: String },
    /// Stage mount-share.sh on the client for the server's share.
    Mount { kind: String, server: String, path: String, client: String },
    /// Stage the mount's removal.
    Unmount { client: String, at: String },
}

impl Ask {
    /// What it will do, in a sentence, for the confirmation.
    pub fn say(&self) -> String {
        match self {
            Ask::Survey => "read every machine's disks, links and shares".into(),
            Ask::Share { machine, path } => format!("share {} on {} over NFS to the fleet's network (rw, root squashed), now and at every boot", path, machine),
            Ask::Mount { kind, server, path, client } => format!("mount {}:{} on {} over {}, under /mnt/fleet, now and at every boot", server, path, client, kind),
            Ask::Unmount { client, at } => format!("unmount {} on {}, and not at boot", at, client),
        }
    }
    fn machine(&self) -> &str {
        match self {
            Ask::Survey => "",
            Ask::Share { machine, .. } => machine,
            Ask::Mount { client, .. } => client,
            Ask::Unmount { client, .. } => client,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub sel: String,
    pub operator: bool,
    /// The Ceph candidates page, in place of the machine.
    pub ceph: bool,
    share: Field,
    /// A change waiting for its OK: staged, then typed in the Sessions tile.
    confirm: Option<Ask>,
    /// The mount kind the patch bay's + uses.
    kind: usize,
    /// What storage.py last said.
    pub said: String,
    pub ask: Option<Ask>,
}

const KINDS: [&str; 3] = ["nfs", "sshfs", "smb"];

impl State {
    pub fn new(operator: bool) -> State {
        State { operator, ..State::default() }
    }
}

fn speed(mbps: i64) -> String {
    if mbps >= 1000 {
        format!("{} Gb", (mbps as f64 / 1000.0 * 10.0).round() / 10.0)
    } else if mbps > 0 {
        format!("{} Mb", mbps)
    } else {
        "?".into()
    }
}

fn size(gib: f64) -> String {
    if gib >= 1000.0 {
        format!("{:.1} TB", gib * 1.073741824 / 1000.0)
    } else {
        format!("{:.0} GB", gib * 1.073741824)
    }
}

pub fn draw(ui: &mut Ui, m: &Model, st: &mut State) -> Option<nav::View> {
    let t = ui.t;
    let chords = st.confirm.is_none();
    let (rest, go, _) = nav::header(ui, nav::View::Storage, chords, "the fleet's disks and shares: survey, share, mount", None);
    let body = rest.inset(PAD);
    if !m.why.is_empty() {
        ui.label_in(body.x, body.y + 8, body.w, &m.why, &F8X13, t.dim);
        if st.operator && ui.button(rect(body.x, body.y + 32, 110, 22), "Survey all", true) {
            st.ask = Some(Ask::Survey);
        }
        return go;
    }
    if st.sel.is_empty() || !m.machines.iter().any(|x| x.name == st.sel) {
        st.sel = m.machines.first().map(|x| x.name.clone()).unwrap_or_default();
    }
    let lh = F8X13.height as i32 + 6;
    // cut_bottom gives the bottom part first.
    let (bay, top) = body.cut_bottom((body.h / 3).max(130));

    // ---- the machines ------------------------------------------------------------
    let lw = 290.min(top.w / 3);
    let mut y = top.y;
    ui.label(top.x, y, "Machines", &F8X13, t.ink);
    if st.operator && chords && ui.button(rect(top.x + lw - 100, y - 4, 100, 22), "Survey all", true) {
        st.ask = Some(Ask::Survey);
    }
    y += lh + 4;
    for x in &m.machines {
        if y + lh * 2 > top.bottom() - lh - 6 {
            break;
        }
        let r = rect(top.x, y - 2, lw, lh * 2 - 4);
        let on = x.name == st.sel && !st.ceph;
        ui.panel(r, if on { t.tile } else { t.base });
        if on {
            ui.outline(r, t.up);
        }
        let cands = x.disks.iter().filter(|d| d.role == "candidate" || d.role == "idle pool").count();
        ui.label_in(r.x + 6, y + 1, lw - 12, &format!("{}  {} · {} GB", x.name, speed(x.mbps), x.mem_gb), &F8X13, if x.error.is_empty() { t.ink } else { t.warn });
        let role = if x.ceph.starts_with("server by hand") { "by hand".to_string() } else { x.ceph.split(':').next().unwrap_or("").split(' ').next().unwrap_or("").to_string() };
        let sub = if !x.error.is_empty() { x.error.clone() } else { format!("{} disks · {} free · Ceph {}", x.disks.len(), cands, role) };
        ui.label_in(r.x + 6, y + lh - 3, lw - 12, &sub, &F8X13, t.dim);
        if chords && ui.button_hit(r) {
            st.sel = x.name.clone();
            st.ceph = false;
        }
        y += lh * 2 - 2;
    }
    let cr = rect(top.x, top.bottom() - lh - 2, lw, lh);
    ui.panel(cr, if st.ceph { t.tile } else { t.base });
    if st.ceph {
        ui.outline(cr, t.up);
    }
    ui.label_in(cr.x + 6, cr.y + 3, lw - 12, &format!("Ceph: {} disks to choose from", m.candidates.len()), &F8X13, t.ink);
    if chords && ui.button_hit(cr) {
        st.ceph = true;
    }

    // ---- the chosen machine, or the Ceph candidates ----------------------------------
    let rx = top.x + lw + 16;
    let rw = top.right() - rx;
    let mut y = top.y;
    if st.ceph {
        ui.label_in(rx, y, rw, "Ceph: disks that could be OSDs, fastest first", &F8X13, t.ink);
        y += lh;
        ui.label_in(rx, y, rw, "Nothing here formats one. Servers: Ubuntu, 4 GB an OSD, 10 Gb.", &F8X13, t.dim);
        y += lh + 2;
        for (mach, d, link) in &m.candidates {
            if y > top.bottom() - lh {
                break;
            }
            let c = if d.role == "idle pool" { t.warn } else { t.up };
            ui.label_in(rx, y, 110, mach, &F8X13, t.ink);
            ui.label_in(rx + 112, y, 60, &d.name, &F8X13, t.ink);
            ui.label_in(rx + 172, y, 46, &d.kind, &F8X13, c);
            ui.label_in(rx + 220, y, 70, &size(d.size_gib), &F8X13, t.ink);
            ui.label_in(rx + 292, y, 110, link, &F8X13, t.dim);
            ui.label_in(rx + 406, y, rw - 406, &d.why, &F8X13, t.dim);
            y += lh - 3;
        }
    } else if let Some(x) = m.machines.iter().find(|x| x.name == st.sel) {
        ui.label_in(rx, y, rw, &format!("{}  {} · {} · surveyed {}", x.name, x.addr, x.os, if x.surveyed.is_empty() { "never".into() } else { x.surveyed.replace('T', " ") }), &F8X13, t.ink);
        y += lh;
        let links: Vec<String> = x.nics.iter().filter(|(_, _, s)| s == "up").map(|(nm, mb, _)| format!("{} {}", nm, speed(*mb))).collect();
        ui.label_in(rx, y, rw, &format!("links: {}   ·   Ceph: {}", if links.is_empty() { "none up".into() } else { links.join(", ") }, x.ceph), &F8X13, t.dim);
        y += lh;
        for p in &x.zpools {
            ui.label_in(rx, y, rw, &format!("ZFS pool {}", p), &F8X13, t.up);
            y += lh - 3;
        }
        for d in &x.disks {
            if y > top.bottom() - lh * 3 {
                break;
            }
            let c = match d.role.as_str() {
                "candidate" => t.up,
                "idle pool" => t.warn,
                "system" | "zfs" | "in use" | "locked" => t.ink,
                _ => t.dim,
            };
            ui.label_in(rx, y, 70, &d.name, &F8X13, t.ink);
            ui.label_in(rx + 72, y, 46, &d.kind, &F8X13, t.dim);
            ui.label_in(rx + 120, y, 70, &size(d.size_gib), &F8X13, t.ink);
            ui.label_in(rx + 192, y, 80, &d.role, &F8X13, c);
            ui.label_in(rx + 274, y, rw - 274, &format!("{}{}", if d.model.is_empty() { String::new() } else { format!("{} · ", d.model) }, d.why), &F8X13, t.dim);
            y += lh - 3;
        }
        for mt in &x.mounts {
            if y > top.bottom() - lh * 3 {
                break;
            }
            ui.label_in(rx, y, rw - 110, &format!("mounted {} at {} ({})", mt.source, mt.at, mt.kind), &F8X13, t.up);
            if st.operator && chords && ui.button(rect(top.right() - 100, y - 3, 100, lh - 2), "Unmount...", true) {
                st.confirm = Some(Ask::Unmount { client: x.name.clone(), at: mt.at.clone() });
            }
            y += lh - 1;
        }
        // Share a path of this machine with the fleet.
        if st.operator {
            let fy = top.bottom() - lh - 2;
            ui.label(rx, fy + 3, "share a path", &F8X13, t.dim);
            let fr = rect(rx + 104, fy, rw - 104 - 110, lh);
            ui.field(fr, &st.share, "/Hexamir/assets");
            let entered = if chords { st.share.feed(&ui.f) } else { false };
            let ok = st.share.text.starts_with('/') && st.share.text.len() > 1;
            if chords && (ui.button(rect(top.right() - 100, fy, 100, lh), "Share...", ok) || entered) && ok {
                st.confirm = Some(Ask::Share { machine: x.name.clone(), path: st.share.text.clone() });
            }
        }
    }

    // ---- the patch bay ---------------------------------------------------------------
    ui.hrule(bay.x, bay.y + 4, bay.w);
    let mut y = bay.y + 12;
    ui.label(bay.x, y, "Patch bay: a row a share, a column a machine", &F8X13, t.ink);
    // The kind the + mounts with.
    let mut kx = bay.right() - 3 * 64;
    for (i, k) in KINDS.iter().enumerate() {
        let b = rect(kx, y - 4, 60, 20);
        let on = i == st.kind;
        ui.panel(b, if on { t.accent } else { t.base });
        ui.outline(b, if on { t.accent } else { t.line });
        ui.label_in(b.x + 8, b.y + 3, 48, k, &F8X13, if on { t.base } else { t.dim });
        if chords && ui.button_hit(b) {
            st.kind = i;
        }
        kx += 64;
    }
    y += lh + 2;
    let names: Vec<&Machine> = m.machines.iter().collect();
    let col0 = 190.min(bay.w / 4);
    let cw = ((bay.w - col0) / names.len().max(1) as i32).clamp(40, 110);
    for (i, x) in names.iter().enumerate() {
        ui.label_in(bay.x + col0 + i as i32 * cw, y, cw - 4, &x.name, &F8X13, t.dim);
    }
    y += lh;
    if m.shares.is_empty() {
        ui.label_in(bay.x, y + 4, bay.w, "No shares yet. Choose a machine above, type a path it holds (a ZFS dataset, a btrfs mount), and Share... it.", &F8X13, t.dim);
    }
    for sh in &m.shares {
        if y > bay.bottom() - lh * 2 {
            break;
        }
        ui.label_in(bay.x, y, col0 - 6, &format!("{}:{}", sh.server, sh.path), &F8X13, t.ink);
        for (i, x) in names.iter().enumerate() {
            let cx = bay.x + col0 + i as i32 * cw;
            if x.name == sh.server {
                ui.label(cx, y, "source", &F8X13, t.accent);
                continue;
            }
            let there = x.mounts.iter().any(|mt| mt.source.ends_with(&format!(":{}", sh.path)) && (mt.source.starts_with(&sh.addr) || mt.source.contains(&format!("@{}", sh.addr))));
            if there {
                ui.label(cx, y, "● mounted", &F8X13, t.up);
            } else if st.operator && chords && ui.button(rect(cx, y - 3, 30, lh - 2), "+", true) {
                st.confirm = Some(Ask::Mount { kind: KINDS[st.kind].into(), server: sh.server.clone(), path: sh.path.clone(), client: x.name.clone() });
            }
        }
        y += lh;
    }
    if !st.said.is_empty() {
        ui.label_in(bay.x, bay.bottom() - lh + 4, bay.w, &st.said, &F8X13, t.dim);
    }

    // ---- the confirmation ------------------------------------------------------------
    if let Some(ask) = st.confirm.clone() {
        let pane = rect(body.x + body.w / 6, body.y + body.h / 5, body.w * 2 / 3, 190);
        ui.panel(pane, t.tile);
        ui.outline(pane, t.accent);
        let ix = pane.x + 16;
        let iw = pane.w - 32;
        let mut y = pane.y + 14;
        ui.label_in(ix, y, iw, "Needs root on the machine", &F10X20, t.ink);
        y += 30;
        ui.label_in(ix, y, iw, &ask.say(), &F8X13, t.ink);
        y += lh + 4;
        ui.label_in(ix, y, iw, &format!("The script is copied to {}'s ~/orrery-recipes, and its command typed into {}'s Sessions tile,", ask.machine(), ask.machine()), &F8X13, t.dim);
        y += lh - 2;
        ui.label_in(ix, y, iw, "where you type the password. Each script is safe to run again; --undo takes it away.", &F8X13, t.dim);
        let by = pane.bottom() - 36;
        if ui.button(rect(ix, by, 110, 24), "Stage it", true) || ui.f.pressed(crate::surface::Sym::Return) {
            st.ask = Some(ask);
            st.confirm = None;
            st.share.clear();
        } else if ui.button(rect(ix + 120, by, 90, 24), "Cancel", true) || ui.f.pressed(crate::surface::Sym::Escape) {
            st.confirm = None;
        }
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_survey_reads_as_machines_shares_and_candidates() {
        let doc = r#"{"machines": [{"name": "boburner", "os": "ubuntu", "addr": "192.168.8.113", "mem_gb": 31, "mbps": 10000,
            "ceph": "server", "nics": [{"name": "enp6s0", "mbps": 10000, "state": "up"}],
            "disks": [{"name": "sda", "size_gib": 3726.0, "kind": "hdd", "role": "zfs", "why": "a member of a ZFS pool"}],
            "zpools": ["Hexamir 7.25T 3.32T 3.93T ONLINE"], "mounts": [], "surveyed": "2026-10-29T20:00:00", "error": "", "log": []},
            {"name": "jaguaribe", "os": "alpine", "addr": "192.168.8.245", "mem_gb": 47, "mbps": 1000, "ceph": "client",
            "nics": [], "disks": [], "zpools": [], "mounts": [{"source": "192.168.8.113:/Hexamir", "at": "/mnt/fleet/boburner-Hexamir", "kind": "nfs4"}],
            "surveyed": "", "error": "", "log": []}],
            "exports": [{"server": "boburner", "addr": "192.168.8.113", "path": "/Hexamir"}],
            "candidates": [{"machine": "jaguaribe", "name": "sdh", "size_gib": 447.1, "kind": "ssd", "role": "candidate", "why": "w", "mbps": 1000, "mem_gb": 47}]}"#;
        let m = Model::parse(doc);
        assert_eq!(m.machines.len(), 2);
        assert_eq!(m.machines[0].mbps, 10000);
        assert_eq!(m.machines[0].disks[0].role, "zfs");
        assert_eq!(m.shares, vec![Share { server: "boburner".into(), addr: "192.168.8.113".into(), path: "/Hexamir".into() }]);
        assert_eq!(m.candidates[0].0, "jaguaribe");
        assert_eq!(m.machines[1].mounts[0].kind, "nfs4");
        assert!(Model::parse("").why.contains("reading"));
    }

    #[test]
    fn sizes_and_speeds_read_as_a_person_says_them() {
        assert_eq!(speed(10000), "10 Gb");
        assert_eq!(speed(2500), "2.5 Gb");
        assert_eq!(speed(100), "100 Mb");
        assert_eq!(size(3726.0), "4.0 TB");
        assert_eq!(size(447.1), "480 GB");
    }
}
