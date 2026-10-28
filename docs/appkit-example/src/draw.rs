//! The drawing: a software canvas, no platform in sight.
//!
//! An orrery: a sun and three bodies on their orbits, turning with time, and
//! a ring where the pointer is. A click stops and starts it.

pub const PAPER: u32 = 0xFCE2AB;
pub const INK: u32 = 0x121212;
pub const LINE: u32 = 0xC4A870;
pub const ACCENT: u32 = 0x87704F;
pub const SUN: u32 = 0xF0A03A;
pub const BRASS: u32 = 0xB06A24;

/// What the drawing remembers between frames.
pub struct State {
    pub pointer: Option<(i32, i32)>,
    pub paused: bool,
    /// Seconds of turning, which stops while paused.
    pub turned: f64,
}

impl Default for State {
    fn default() -> Self {
        State { pointer: None, paused: false, turned: 0.0 }
    }
}

/// Fill a disc, clipped to the canvas.
fn disc(px: &mut [u32], w: usize, h: usize, cx: f64, cy: f64, r: f64, c: u32) {
    let (x0, x1) = ((cx - r).floor().max(0.0) as usize, ((cx + r).ceil() as usize).min(w));
    let (y0, y1) = ((cy - r).floor().max(0.0) as usize, ((cy + r).ceil() as usize).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                px[y * w + x] = c;
            }
        }
    }
}

/// An ellipse's outline, as dots close enough to read as a line.
fn ring(px: &mut [u32], w: usize, h: usize, cx: f64, cy: f64, a: f64, b: f64, c: u32) {
    let steps = (a.max(b) * 8.0) as usize;
    for i in 0..steps {
        let t = i as f64 / steps as f64 * std::f64::consts::TAU;
        let (x, y) = (cx + a * t.cos(), cy + b * t.sin());
        if x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h {
            px[y as usize * w + x as usize] = c;
        }
    }
}

/// One frame, from the state alone. Called by the window and by `--frame`.
pub fn frame(px: &mut [u32], w: usize, h: usize, st: &State) {
    px.fill(PAPER);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let unit = w.min(h) as f64 / 400.0;
    // (orbit radius, period in seconds, body radius, colour)
    let bodies = [(70.0, 4.0, 8.0, ACCENT), (120.0, 9.0, 12.0, INK), (175.0, 17.0, 7.0, BRASS)];
    for &(r, _, _, _) in &bodies {
        ring(px, w, h, cx, cy, r * unit, r * unit * 0.45, LINE);
    }
    disc(px, w, h, cx, cy, 30.0 * unit, BRASS);
    disc(px, w, h, cx, cy, 26.0 * unit, SUN);
    for &(r, period, size, c) in &bodies {
        let t = st.turned / period * std::f64::consts::TAU;
        let (x, y) = (cx + r * unit * t.cos(), cy + r * unit * 0.45 * t.sin());
        disc(px, w, h, x, y, size * unit, c);
    }
    if let Some((x, y)) = st.pointer {
        ring(px, w, h, x as f64, y as f64, 14.0 * unit, 14.0 * unit, if st.paused { BRASS } else { ACCENT });
    }
}
