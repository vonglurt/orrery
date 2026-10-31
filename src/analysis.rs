//! The Algorhythm page's analysis tabs (Paul, 30 October: "a terrain style 3d graph";
//! "something like a spectrogram"; "laying out the input sensor as it would be seen";
//! "redraw how the new fleet orchestrates").
//!
//! IT DRAWS AND DECIDES NOTHING: `tools/analysis.py` writes the numbers, in the background,
//! from the fleet's own ledger and nets. A terrain here is a kernel average of reads that
//! moved several dials at once, and says so, and dims where few reads stand: A PEAK ON NO
//! DATA IS THE EASIEST LIE A LANDSCAPE CAN TELL. It is a map of where to look. The census
//! is still what proves anything.

use crate::draw::Rgb;
use crate::font::{F10X20, F8X13};
use crate::json::Value;
use crate::ui::{rect, Rect, Ui};

pub const TABS: [&str; 6] = ["Patch", "Terrain", "Spectrum", "Matrices", "Senses", "Fleet"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub tab: usize,
    pub pair: usize,
    /// Quarter-turns of a sixteenth each.
    pub yaw: i32,
    pub hybrid: bool,
}

fn blend(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 255) as f64, ((b >> s) & 255) as f64);
        ((x + (y - x) * t) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_num()).unwrap_or(0.0)
}
fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(|x| x.as_array()).map(|a| &a[..]).unwrap_or(&[])
}
fn nums(v: &Value) -> Vec<f64> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_num()).collect()).unwrap_or_default()
}
fn grid(v: &[Value]) -> Vec<Vec<f64>> {
    v.iter().map(nums).collect()
}

/// A diverging colour: red under zero, green over, by the share of `max`.
fn heat(ui: &Ui, v: f64, max: f64) -> Rgb {
    let t = ui.t;
    let k = (v.abs() / max.max(1e-9)).sqrt();
    blend(t.tile, if v >= 0.0 { t.up } else { t.alarm }, k)
}

/// The tab strip; returns what is left under it.
pub fn tabs(ui: &mut Ui, at: Rect, st: &mut State) -> Rect {
    let w = 96;
    for (i, name) in TABS.iter().enumerate() {
        let r = rect(at.x + i as i32 * (w + 4), at.y, w, 24);
        let on = st.tab == i;
        if on {
            ui.c.rect(r.x + 4, r.bottom(), r.w - 8, 2, ui.t.up);
        }
        if ui.button(r, name, !on) {
            st.tab = i;
        }
    }
    rect(at.x, at.y + 32, at.w, at.h - 32)
}

/// A filled triangle, one row of pixels at a time: the canvas has rects and discs.
fn tri(ui: &mut Ui, p: [(f64, f64); 3], c: Rgb) {
    let y0 = p.iter().map(|q| q.1).fold(f64::MAX, f64::min).ceil() as i32;
    let y1 = p.iter().map(|q| q.1).fold(f64::MIN, f64::max).floor() as i32;
    for y in y0..=y1 {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 0..3 {
            let (a, b) = (p[i], p[(i + 1) % 3]);
            let yy = y as f64;
            if (a.1 <= yy && b.1 >= yy) || (b.1 <= yy && a.1 >= yy) {
                let x = if (b.1 - a.1).abs() < 1e-6 { a.0 } else { a.0 + (yy - a.1) * (b.0 - a.0) / (b.1 - a.1) };
                lo = lo.min(x.min(if (b.1 - a.1).abs() < 1e-6 { b.0 } else { x }));
                hi = hi.max(x.max(if (b.1 - a.1).abs() < 1e-6 { b.0 } else { x }));
            }
        }
        if hi >= lo {
            ui.c.rect(lo.floor() as i32, y, (hi - lo).ceil() as i32 + 1, 1, c);
        }
    }
}

/// Grid point to the screen: turned by the yaw, tipped back, raised by its height.
pub fn project(u: f64, v: f64, h: f64, yaw: f64, cx: f64, cy: f64, r: f64) -> (f64, f64, f64) {
    let (x, y) = (u * yaw.cos() - v * yaw.sin(), u * yaw.sin() + v * yaw.cos());
    (cx + x * r, cy + y * r * 0.5 - h, y)
}

fn draw_terrain(ui: &mut Ui, at: Rect, a: &Value, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 5;
    let list = arr(a, "terrain");
    if list.is_empty() {
        ui.label_in(at.x, at.y, at.w, "Not enough reads yet to draw a terrain.", &F8X13, t.dim);
        return;
    }
    st.pair = st.pair.min(list.len() - 1);
    let w = 230;
    let side = rect(at.right() - w, at.y, w, at.h);
    // The pairs.
    let mut y = side.y;
    ui.label(side.x, y, "Two dials at a time", &F8X13, t.dim);
    y += lh;
    for (i, p) in list.iter().enumerate() {
        let r = rect(side.x, y, w, lh + 2);
        let on = i == st.pair;
        ui.panel(r, if on { t.tile } else { t.base });
        if on {
            ui.outline(r, t.up);
        }
        ui.label_in(r.x + 5, r.y + 3, w - 10, &format!("{} x {}", s(p, "a"), s(p, "b")), &F8X13, if on { t.ink } else { t.dim });
        if ui.button_hit(r) {
            st.pair = i;
        }
        y += lh + 4;
    }
    y += 4;
    if ui.button(rect(side.x, y, 72, 22), "Turn <", true) {
        st.yaw -= 1;
    }
    if ui.button(rect(side.x + 78, y, 72, 22), "Turn >", true) {
        st.yaw += 1;
    }
    y += 30;
    let p = &list[st.pair];
    let peaks = arr(p, "peaks");
    ui.label(side.x, y, if peaks.is_empty() { "No local maximum here" } else { "Local maxima, best first" }, &F8X13, if peaks.is_empty() { t.dim } else { t.ink });
    y += lh;
    for (i, q) in peaks.iter().take(5).enumerate() {
        ui.label_in(side.x, y, w, &format!("{}  {:+.2} at {:.3}, {:.3}", i + 1, n(q, "z"), n(q, "a"), n(q, "b")), &F8X13, t.ink);
        y += lh - 2;
        ui.label_in(side.x + 22, y, w - 22, &format!("weight of reads {:.0}", n(q, "support")), &F8X13, t.dim);
        y += lh - 1;
    }
    y += 4;
    for line in ["Ring: today's defaults.", "Dim: few reads there.", "A map of where to look.", "Only a census proves one."] {
        ui.label_in(side.x, y, w, line, &F8X13, t.dim);
        y += lh - 2;
    }

    // The surface.
    let plot = rect(at.x, at.y, at.w - w - 14, at.h);
    let z = grid(arr(p, "z"));
    let sup = grid(arr(p, "support"));
    let rows = z.len();
    let cols = z.first().map(|r| r.len()).unwrap_or(0);
    if rows < 2 || cols < 2 {
        return;
    }
    ui.label_in(plot.x, plot.y, plot.w, &format!("Gain, floored at -3, over today's defaults, points a seat: {} reads over {} and {}", n(p, "reads"), s(p, "a"), s(p, "b")), &F8X13, t.ink);
    // A floor at three points under today: the valleys reach -7, and on their scale the ground
    // that matters, within a point of today, drew flat and black.
    let z: Vec<Vec<f64>> = z.iter().map(|row| row.iter().map(|v| v.max(-3.0)).collect()).collect();
    let zmax = z.iter().flatten().fold(0.5f64, |m, v| m.max(v.abs()));
    let yaw = 0.7 + st.yaw as f64 * std::f64::consts::PI / 8.0;
    let at_uv = |ix: usize, iy: usize| (2.0 * ix as f64 / (cols - 1) as f64 - 1.0, 2.0 * iy as f64 / (rows - 1) as f64 - 1.0);
    // Fitted to the panel: a deep valley hung off the bottom of a fixed scale. Measured at unit
    // size, then scaled so the whole surface, heights and all, sits between the text.
    let hu = 0.55 / zmax;
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (j, row) in z.iter().enumerate() {
        for (i, v) in row.iter().enumerate() {
            let (u, w) = at_uv(i, j);
            let g = project(u, w, v * hu, yaw, 0.0, 0.0, 1.0);
            x0 = x0.min(g.0);
            x1 = x1.max(g.0);
            y0 = y0.min(g.1);
            y1 = y1.max(g.1);
        }
    }
    let area = rect(plot.x, plot.y + lh + 30, plot.w, plot.h - lh * 3 - 40);
    let r = (area.w as f64 / (x1 - x0).max(1e-6)).min(area.h as f64 / (y1 - y0).max(1e-6));
    let hs = hu * r;
    let cx = area.x as f64 + area.w as f64 / 2.0 - r * (x0 + x1) / 2.0;
    let cy = area.y as f64 + area.h as f64 / 2.0 - r * (y0 + y1) / 2.0;
    // Back to front, the painter's way.
    let mut cells: Vec<(f64, usize, usize)> = Vec::with_capacity(rows * cols);
    for iy in 0..rows - 1 {
        for ix in 0..cols - 1 {
            let (u, v) = at_uv(ix, iy);
            cells.push((project(u + 1.0 / cols as f64, v + 1.0 / rows as f64, 0.0, yaw, cx, cy, r).2, ix, iy));
        }
    }
    cells.sort_by(|a, b| a.0.total_cmp(&b.0));
    let sup_max = sup.iter().flatten().fold(1e-6f64, |m, v| m.max(*v));
    for (_, ix, iy) in cells {
        let c = [(ix, iy), (ix + 1, iy), (ix + 1, iy + 1), (ix, iy + 1)];
        let q: Vec<(f64, f64)> = c
            .iter()
            .map(|&(i, j)| {
                let (u, v) = at_uv(i, j);
                let g = project(u, v, z[j][i] * hs, yaw, cx, cy, r);
                (g.0, g.1)
            })
            .collect();
        let mean = c.iter().map(|&(i, j)| z[j][i]).sum::<f64>() / 4.0;
        // Terraces each quarter point: contour lines without drawing any.
        let band = (mean * 4.0).round() / 4.0;
        // Lit from one side, so a slope shows on a surface of one colour.
        let slope = (z[iy][ix + 1] - z[iy][ix]) + (z[iy + 1][ix] - z[iy][ix]);
        let lit = (0.5 + slope / zmax * 4.0).clamp(0.0, 1.0);
        let base = heat(ui, band, zmax);
        let shaded = if lit > 0.5 { blend(base, t.ink, (lit - 0.5) * 0.35) } else { blend(base, t.base, (0.5 - lit) * 0.7) };
        let seen = (sup[iy][ix] / (sup_max * 0.08)).clamp(0.12, 1.0);
        let col = blend(t.tile, shaded, seen);
        tri(ui, [q[0], q[1], q[2]], col);
        tri(ui, [q[0], q[2], q[3]], col);
    }
    // Today's defaults: 0 doublings on each axis.
    let (ax, ay) = (nums(p.get("ax").unwrap_or(&Value::Null)), nums(p.get("ay").unwrap_or(&Value::Null)));
    if ax.len() == 2 && ay.len() == 2 {
        let (u, v) = (2.0 * (0.0 - ax[0]) / (ax[1] - ax[0]) - 1.0, 2.0 * (0.0 - ay[0]) / (ay[1] - ay[0]) - 1.0);
        let g = project(u, v, 0.0, yaw, cx, cy, r);
        ui.c.disc(g.0 as i32, g.1 as i32, 6, t.ink);
        ui.c.disc(g.0 as i32, g.1 as i32, 3, t.base);
    }
    for (i, q) in peaks.iter().take(5).enumerate() {
        let (ix, iy) = (n(q, "ix") as usize, n(q, "iy") as usize);
        let (u, v) = at_uv(ix, iy);
        let g = project(u, v, n(q, "z") * hs, yaw, cx, cy, r);
        ui.c.rect(g.0 as i32, g.1 as i32 - 24, 1, 24, t.warn);
        ui.c.disc(g.0 as i32, g.1 as i32 - 28, 8, t.warn);
        ui.label(g.0 as i32 - 3, g.1 as i32 - 34, &(i + 1).to_string(), &F8X13, t.base);
    }
    // The corners, so the axes can be read: each dial's range, in real values.
    if let Some(today) = p.get("today").map(nums).filter(|x| x.len() == 2) {
        let lab = plot.bottom() - lh;
        ui.label_in(plot.x, lab - lh, plot.w, &format!("{}: {:.3} to {:.3}", s(p, "a"), today[0] * 2f64.powf(ax[0]), today[0] * 2f64.powf(ax[1])), &F8X13, t.dim);
        ui.label_in(plot.x, lab, plot.w, &format!("{}: {:.3} to {:.3}", s(p, "b"), today[1] * 2f64.powf(ay[0]), today[1] * 2f64.powf(ay[1])), &F8X13, t.dim);
    }
}

/// A heat map of `cells`, row 0 at the top.
fn heatmap(ui: &mut Ui, r: Rect, cells: &[Vec<f64>], colour: impl Fn(&Ui, f64) -> Rgb) {
    let rows = cells.len() as i32;
    let cols = cells.first().map(|c| c.len()).unwrap_or(0) as i32;
    if rows == 0 || cols == 0 {
        return;
    }
    for y in 0..rows {
        for x in 0..cols {
            let (x0, x1) = (r.x + x * r.w / cols, r.x + (x + 1) * r.w / cols);
            let (y0, y1) = (r.y + y * r.h / rows, r.y + (y + 1) * r.h / rows);
            let c = colour(ui, cells[y as usize][x as usize]);
            ui.c.rect(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1), c);
        }
    }
    ui.outline(r, ui.t.line);
}

fn line(ui: &mut Ui, r: Rect, v: &[f64], c: Rgb) {
    let max = v.iter().fold(1e-6f64, |m, x| m.max(*x));
    let at = |i: usize| {
        let px = r.x + i as i32 * (r.w - 2) / (v.len() as i32 - 1).max(1);
        (px, r.bottom() - 2 - (v[i] / max * (r.h - 4) as f64) as i32)
    };
    // Joined point to point: a rect a point read as a dashed line once the points ran sparse.
    for i in 0..v.len().saturating_sub(1) {
        let ((x0, y0), (x1, y1)) = (at(i), at(i + 1));
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for k in 0..=steps {
            ui.c.rect(x0 + (x1 - x0) * k / steps, y0 + (y1 - y0) * k / steps, 2, 2, c);
        }
    }
}

fn draw_spectrum(ui: &mut Ui, at: Rect, a: &Value, st: &mut State) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 5;
    let mut y = at.y;
    if ui.button(rect(at.x, y, 90, 22), "Hybrid", !st.hybrid) {
        st.hybrid = true;
    }
    if ui.button(rect(at.x + 96, y, 90, 22), "Plain", st.hybrid) {
        st.hybrid = false;
    }
    let spec = a.get("spectrum").and_then(|x| x.get(if st.hybrid { "hybrid" } else { "plain" }));
    ui.label_in(at.x + 200, y + 4, at.w - 200, "A net's spectrogram: its middle layer's singular values, a column a champion, oldest at the left", &F8X13, t.dim);
    y += 30;
    let h = (at.h - 30) / 2 - lh;
    match spec.filter(|x| !arr(x, "columns").is_empty()) {
        None => {
            ui.label_in(at.x, y, at.w, "No champions of this kind yet.", &F8X13, t.dim);
        }
        Some(sp) => {
            let cols = grid(arr(sp, "columns"));
            let rows = cols.first().map(|c| c.len()).unwrap_or(0);
            let flip: Vec<Vec<f64>> = (0..rows).map(|r| cols.iter().map(|c| c[r]).collect()).collect();
            let max = flip.iter().flatten().fold(1e-6f64, |m, v| m.max(*v));
            let w = at.w * 3 / 5;
            ui.label_in(at.x, y, w, &format!("{} champions, {} shown; strongest direction at the top", n(sp, "champions"), n(sp, "shown")), &F8X13, t.ink);
            let r = rect(at.x, y + lh, w, h - lh);
            heatmap(ui, r, &flip, move |u, v| blend(u.t.tile, u.t.up, (v / max).sqrt()));
            let rx = r.right() + 14;
            let rw = at.right() - rx;
            let (norm, drift) = (nums(sp.get("norm").unwrap_or(&Value::Null)), nums(sp.get("drift").unwrap_or(&Value::Null)));
            ui.label_in(rx, y, rw, &format!("size of all its weights: now {:.0}", norm.last().copied().unwrap_or(0.0)), &F8X13, t.up);
            let c1 = rect(rx, y + lh, rw, (h - lh) / 2 - lh);
            ui.panel(c1, t.tile);
            line(ui, c1, &norm, t.up);
            let y2 = c1.bottom() + 4;
            ui.label_in(rx, y2, rw, &format!("distance from where it began: now {:.0}", drift.last().copied().unwrap_or(0.0)), &F8X13, t.accent);
            let c2 = rect(rx, y2 + lh, rw, r.bottom() - y2 - lh);
            ui.panel(c2, t.tile);
            line(ui, c2, &drift, t.accent);
        }
    }
    y += h + 6;
    // The dials, promotion by promotion.
    if let Some(d) = a.get("dials") {
        let cells = grid(arr(d, "cells"));
        let names = arr(d, "dials");
        let commits = arr(d, "commits");
        ui.label_in(at.x, y, at.w, "The dials over the promotions, in doublings from before the first: left to right, oldest first", &F8X13, t.ink);
        y += lh;
        let lw = 150;
        let r = rect(at.x + lw, y, at.w - lw, (at.bottom() - y - lh).max(20));
        let max = cells.iter().flatten().fold(0.1f64, |m, v| m.max(v.abs()));
        heatmap(ui, r, &cells, move |u, v| heat(u, v, max));
        let rows = cells.len().max(1) as i32;
        for (i, nm) in names.iter().enumerate() {
            if r.h / rows >= F8X13.height as i32 {
                ui.label_in(at.x, r.y + i as i32 * r.h / rows, lw - 6, nm.as_str().unwrap_or(""), &F8X13, t.dim);
            }
        }
        for (i, c) in commits.iter().enumerate() {
            ui.label_in(r.x + i as i32 * r.w / commits.len().max(1) as i32 + 2, r.bottom() + 2, r.w / commits.len().max(1) as i32, c.as_str().unwrap_or(""), &F8X13, t.dim);
        }
    }
}

fn draw_matrices(ui: &mut Ui, at: Rect, a: &Value) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 5;
    let Some(net) = a.get("net") else {
        ui.label_in(at.x, at.y, at.w, "No net to read yet.", &F8X13, t.dim);
        return;
    };
    let mut y = at.y;
    ui.label_in(at.x, y, at.w, &format!("The tables of {}, read as linear maps", s(net, "name")), &F8X13, t.ink);
    y += lh + 2;
    let cols = [(0, "layer"), (120, "size"), (200, "largest stretch"), (330, "total size"), (430, "stable rank"), (540, "effective rank"), (670, "near zero")];
    for (x, h) in cols {
        ui.label(at.x + x, y, h, &F8X13, t.dim);
    }
    y += lh;
    let layers = arr(net, "layers");
    for (i, l) in layers.iter().enumerate() {
        let vals = [
            format!("{}", i + 1),
            format!("{} x {}", n(l, "cols"), n(l, "rows")),
            format!("{:.1}", n(l, "spectral")),
            format!("{:.1}", n(l, "fro")),
            format!("{:.1}", n(l, "stable_rank")),
            format!("{:.0} of {:.0}", n(l, "effective_rank"), n(l, "rows").min(n(l, "cols"))),
            format!("{:.0}%", 100.0 * n(l, "sparsity")),
        ];
        for ((x, _), v) in cols.iter().zip(vals.iter()) {
            ui.label(at.x + x, y, v, &F8X13, t.ink);
        }
        y += lh;
    }
    y += 6;
    let half = (at.w - 16) / 2;
    let h = at.bottom() - y - lh;
    // The singular values, a scree plot a layer.
    ui.label_in(at.x, y, half, "Singular values by layer: how many directions each uses", &F8X13, t.dim);
    let scree = rect(at.x, y + lh, half, h);
    ui.panel(scree, t.tile);
    let colours = [t.up, t.accent, t.warn];
    for (i, l) in layers.iter().enumerate() {
        let sv = l.get("sv").map(nums).unwrap_or_default();
        let max = sv.first().copied().unwrap_or(1.0).max(1e-6);
        let mut last = None;
        for (k, v) in sv.iter().enumerate() {
            let px = scree.x + 6 + k as i32 * (scree.w - 12) / sv.len().max(1) as i32;
            // A log scale, 3 decades: the tail is the story.
            let f = ((v / max).max(1e-3).log10() + 3.0) / 3.0;
            let py = scree.bottom() - 6 - (f * (scree.h - 12) as f64) as i32;
            if let Some((ox, oy)) = last {
                let steps = ((px - ox as i32).abs()).max((py - oy as i32).abs()).max(1);
                for s in 0..=steps {
                    ui.c.rect(ox + (px - ox) * s / steps, oy + (py - oy) * s / steps, 2, 2, colours[i % 3]);
                }
            }
            last = Some((px, py));
        }
    }
    // The transform end to end.
    let rx = at.x + half + 16;
    ui.label_in(rx, y, half, "W3 W2 W1: what each group of senses reaches, unbent", &F8X13, t.dim);
    let cells = grid(arr(net, "end_grid"));
    let (ins, outs) = (arr(net, "inputs"), arr(net, "outputs"));
    let lw = 56; // room for "signal"
    let r = rect(rx + lw, y + lh, half - lw, h - lh);
    let max = cells.iter().flatten().fold(1e-6f64, |m, v| m.max(*v));
    heatmap(ui, r, &cells, move |u, v| blend(u.t.tile, u.t.up, (v / max).sqrt()));
    for (i, o) in outs.iter().enumerate() {
        let py = r.y + i as i32 * r.h / outs.len().max(1) as i32 + 3;
        ui.label_in(rx, py, lw - 4, o.as_str().unwrap_or(""), &F8X13, t.dim);
    }
    for (i, o) in ins.iter().enumerate() {
        let w = r.w / ins.len().max(1) as i32;
        ui.label_in(r.x + i as i32 * w + 2, r.bottom() + 2, w - 2, o.as_str().unwrap_or(""), &F8X13, t.dim);
    }
}

/// The senses as the tank has them: the compass round it, its own forward at the top, a ring
/// a channel, and the rest of the inputs as the instruments they are.
fn draw_senses(ui: &mut Ui, at: Rect, a: &Value) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 5;
    let Some(net) = a.get("net") else {
        ui.label_in(at.x, at.y, at.w, "No net to read yet.", &F8X13, t.dim);
        return;
    };
    let listen = net.get("listen").map(nums).unwrap_or_default();
    if listen.len() < 143 {
        return;
    }
    let names = arr(a, "channels");
    ui.label_in(at.x, at.y, at.w, &format!("What {} listens to, laid out as the tank has it: brighter is a stronger first layer", s(net, "name")), &F8X13, t.ink);
    let max = listen.iter().fold(1e-6f64, |m, v| m.max(*v));
    let size = (at.h - lh * 2).min(at.w * 3 / 5);
    let (cx, cy) = (at.x + size / 2, at.y + lh * 2 + size / 2 - lh / 2);
    let rmax = (size / 2 - 14) as f64;
    let rings = 11;
    let inner = rmax * 0.14;
    // The polar grid, a dot at a time: sectors a wedge, channels a ring, the first one inside.
    let (mut py, ph) = (-(size / 2), size / 2);
    while py < ph {
        let mut px = -(size / 2);
        while px < size / 2 {
            let d = ((px * px + py * py) as f64).sqrt();
            if d >= inner && d < rmax {
                let ring = (((d - inner) / (rmax - inner)) * rings as f64) as usize;
                // Forward up, and clockwise on screen as the bearing grows.
                let ang = (px as f64).atan2(-(py as f64));
                let k = ((ang / (std::f64::consts::TAU / 8.0)).round() as i32).rem_euclid(8) as usize;
                let v = listen[k * 11 + ring.min(10)];
                ui.c.rect(cx + px, cy + py, 2, 2, blend(t.tile, t.up, (v / max).sqrt()));
            }
            px += 2;
        }
        py += 2;
    }
    // The tank, pointing forward.
    ui.c.disc(cx, cy, 7, t.ink);
    ui.c.rect(cx - 1, cy - 16, 3, 12, t.ink);
    ui.label(cx - 20, cy - (size / 2) - 2, "ahead", &F8X13, t.dim);
    // The rings' names, and the rest of the inputs.
    let x = at.x + size + 18;
    let w = at.right() - x;
    let mut y = at.y + lh;
    ui.label_in(x, y, w, "Rings, from the tank outward", &F8X13, t.dim);
    y += lh - 2;
    let per: Vec<f64> = (0..11).map(|c| (0..8).map(|k| listen[k * 11 + c]).sum::<f64>() / 8.0).collect();
    let ring_max = per.iter().fold(1e-6f64, |m, v| m.max(*v));
    for (c, nm) in names.iter().enumerate().take(11) {
        ui.label_in(x, y, 146, &format!("{} {}", c + 1, nm.as_str().unwrap_or("")), &F8X13, t.ink);
        let bw = ((w - 150) as f64 * per[c] / ring_max) as i32;
        ui.c.rect(x + 150, y + 3, bw.max(1), 9, t.up);
        y += lh - 3;
    }
    y += 6;
    ui.label_in(x, y, w, "The instruments, each number a bar", &F8X13, t.dim);
    y += lh - 2;
    let mut from = 88;
    for (nm, count) in [("body", 12), ("navigation", 6), ("contacts", 18), ("hints", 9), ("shells", 5), ("air", 5)] {
        ui.label_in(x, y + 2, 90, nm, &F8X13, t.ink);
        let bw = ((w - 96) / count).clamp(2, 18);
        for i in 0..count {
            let v = listen[from + i as usize];
            let h = (v / max * (lh - 2) as f64).max(1.0) as i32;
            ui.c.rect(x + 96 + i * bw, y + lh - 2 - h, (bw - 1).max(1), h, t.up);
        }
        from += count as usize;
        y += lh + 2;
    }
}

/// How the fleet orchestrates now: the three roads to a better brain, and the one way onto main.
fn draw_fleet(ui: &mut Ui, at: Rect, a: &Value) {
    let t = ui.t;
    let lh = F8X13.height as i32 + 5;
    let null = Value::Null;
    let f = a.get("fleet").unwrap_or(&null);
    let box_ = |ui: &mut Ui, r: Rect, title: &str, l1: String, l2: String, hot: bool| {
        ui.panel(r, t.tile);
        ui.outline(r, if hot { t.up } else { t.line });
        ui.label_in(r.x + 8, r.y + 5, r.w - 16, title, &F8X13, t.ink);
        ui.label_in(r.x + 8, r.y + 5 + lh, r.w - 16, &l1, &F8X13, t.dim);
        ui.label_in(r.x + 8, r.y + 5 + lh * 2, r.w - 16, &l2, &F8X13, t.dim);
    };
    let arrow = |ui: &mut Ui, x0: i32, y0: i32, x1: i32, y1: i32| {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for s in 0..=steps {
            ui.c.rect(x0 + (x1 - x0) * s / steps, y0 + (y1 - y0) * s / steps, 2, 2, ui.t.accent);
        }
        ui.c.disc(x1, y1, 4, ui.t.accent);
    };
    ui.label_in(at.x, at.y, at.w, "One planner on this Mac, the machines working what it asks, and one way onto main", &F8X13, t.ink);
    let top = at.y + lh + 4;
    let bh = lh * 3 + 12;
    let gap = 26;
    // The rows share what height there is, leaving room for the line under them.
    let vgap = ((at.h - lh * 3 - 16 - bh * 4) / 3).max(12);
    let w = (at.w - gap * 4) / 5;
    let col = |i: i32| at.x + i * (w + gap);
    // Where trials come from.
    let rows = [
        ("The fit (suggest.py)", "dials: curves over reads".to_string(), "a new generation when low".to_string()),
        ("A/B (ab_trials)", "each switch and version".to_string(), "whole numbers only".to_string()),
        ("Transform (patch runs)", format!("step {:.0} of {:.0}", n(f, "transform_step"), n(f, "transform_of")), format!("{:.0} slots filled", n(f, "patches"))),
    ];
    let mut src = Vec::new();
    for (i, (ti, a1, b1)) in rows.iter().enumerate() {
        let r = rect(col(0), top + i as i32 * (bh + vgap), w, bh);
        box_(ui, r, ti, a1.clone(), b1.clone(), true);
        src.push(r);
    }
    let mid = top + bh + vgap;
    let road = [
        ("First read: 55 seeds", format!("{:.0} waiting on a read", n(f, "reading")), format!("{:.0} dropped", n(f, "dropped"))),
        ("Read on: to 418 seeds", format!("{:.0} reading on", n(f, "read_on")), format!("{:.0} passed", n(f, "passed"))),
        ("Held out: 400 + 400", "second read only if the".to_string(), format!("first is 0+; {:.0} waiting", n(f, "held_waiting"))),
        ("Families and tests", "on a branch first".to_string(), format!("{:.0} promoted to main", n(f, "promotions"))),
    ];
    let mut prev: Option<Rect> = None;
    for (i, (ti, a1, b1)) in road.iter().enumerate() {
        let r = rect(col(i as i32 + 1), mid, w, bh);
        box_(ui, r, ti, a1.clone(), b1.clone(), i == 3);
        if let Some(p) = prev {
            arrow(ui, p.right() + 2, p.y + p.h / 2, r.x - 5, r.y + r.h / 2);
        }
        prev = Some(r);
    }
    for r in &src {
        arrow(ui, r.right() + 2, r.y + r.h / 2, col(1) - 5, mid + bh / 2);
    }
    // Under it: the rest of the machine.
    let y2 = top + (bh + vgap) * 3;
    let lower = [
        ("Crucible, GPU islands", format!("{:.0} islands, {:.0} segments", n(f, "islands"), n(f, "segments")), format!("milestones: {:.0}; not treadcore", n(f, "milestones"))),
        ("Fleet Chat and verbs", "certificates, forced command".to_string(), "orrery hosts the room".to_string()),
        ("Keeper and lanes", "a lane a machine; retried".to_string(), "new code restarts it".to_string()),
        ("Retired from rotation", "train school record spsa tune".to_string(), "parts switches round robin".to_string()),
    ];
    let lw = (at.w - gap * 3) / 4;
    for (i, (ti, a1, b1)) in lower.iter().enumerate() {
        let r = rect(at.x + i as i32 * (lw + gap), y2, lw, bh);
        box_(ui, r, ti, a1.clone(), b1.clone(), i < 3);
    }
    ui.label_in(at.x, y2 + bh + 8, at.w, &format!("Counted at {} from {} reads in the ledger.", s(a, "at"), n(a, "reads")), &F10X20, t.dim);
}

/// The tab shown, in `at`. False: the caller draws the Patch tab.
pub fn draw(ui: &mut Ui, at: Rect, a: Option<&Value>, st: &mut State) -> bool {
    if st.tab == 0 {
        return false;
    }
    let Some(a) = a else {
        ui.label_in(at.x, at.y, at.w, "The analysis is being worked out; it takes about a minute.", &F8X13, ui.t.dim);
        return true;
    };
    match st.tab {
        1 => draw_terrain(ui, at, a, st),
        2 => draw_spectrum(ui, at, a, st),
        3 => draw_matrices(ui, at, a),
        4 => draw_senses(ui, at, a),
        _ => draw_fleet(ui, at, a),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_point_draws_higher_and_a_turn_keeps_the_centre() {
        let low = project(0.2, 0.3, 0.0, 0.7, 100.0, 100.0, 50.0);
        let high = project(0.2, 0.3, 20.0, 0.7, 100.0, 100.0, 50.0);
        assert_eq!(high.1, low.1 - 20.0);
        assert_eq!(project(0.0, 0.0, 0.0, 2.1, 100.0, 80.0, 50.0).0, 100.0);
    }

    #[test]
    fn a_blend_runs_from_one_colour_to_the_other() {
        assert_eq!(blend(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(blend(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(blend(0x102030, 0x102030, 0.5), 0x102030);
    }
}
