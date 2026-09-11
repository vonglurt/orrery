//! Putting a node's screen into a terminal.
//!
//! Two renderers, because "does this terminal do graphics" has no portable
//! answer and the seat should not be useless when the answer is no.
//!
//!   sixel      real pixels. iTerm2, kitty, foot, mlterm, xterm -ti 340.
//!   halfblock  two pixels per cell using U+2580 and 24-bit colour, which
//!              every terminal written this century can do.
//!
//! HALF-BLOCKS ARE THE DEFAULT AND SIXEL IS THE UPGRADE, which is the opposite
//! of what it looks like it should be. A seat that renders nothing on the
//! operator's actual terminal is worse than one that renders coarsely
//! everywhere, and half a cell is enough resolution to see which window has
//! focus and where the pointer is -- which is what Control is for. Sixel is
//! there for when you want to read the text on the node's screen.
//!
//! No dependency, again, and here that is not even a sacrifice: sixel is a
//! 1970s escape sequence and the encoder is arithmetic.

use crate::rfb::Screen;

/// How to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Sixel,
    HalfBlock,
}

impl Mode {
    /// What this terminal can probably do.
    ///
    /// This is a guess from `$TERM` and friends, and it is deliberately a
    /// PESSIMISTIC one: guessing sixel wrong fills the operator's terminal
    /// with garbage they then have to reset, and guessing half-blocks wrong
    /// costs them some resolution. `--sixel` overrides it.
    pub fn detect() -> Mode {
        let term = std::env::var("TERM").unwrap_or_default();
        let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        let sixel = term.contains("sixel")
            || term.starts_with("foot")
            || term.starts_with("mlterm")
            || term.starts_with("yaft")
            || program == "iTerm.app"
            || program == "WezTerm";
        if sixel {
            Mode::Sixel
        } else {
            Mode::HalfBlock
        }
    }
}

/// Nearest-neighbour scale to `(dw, dh)`.
///
/// Nearest and not an average, on purpose. A node's screen is text and window
/// edges, and averaging turns a one-pixel border into a grey smear that reads
/// as blur rather than as a line. Sharp and slightly wrong beats soft.
fn scale(src: &Screen, dw: usize, dh: usize) -> Screen {
    let mut out = Screen::new(dw, dh);
    if dw == 0 || dh == 0 || src.w == 0 || src.h == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = y * src.h / dh;
        for x in 0..dw {
            let sx = x * src.w / dw;
            let (r, g, b) = src.get(sx, sy);
            let i = (y * dw + x) * 3;
            out.px[i] = r;
            out.px[i + 1] = g;
            out.px[i + 2] = b;
        }
    }
    out
}

/// The largest `(w, h)` that fits in `(mw, mh)` keeping `src`'s aspect ratio.
pub fn fit(src_w: usize, src_h: usize, mw: usize, mh: usize) -> (usize, usize) {
    if src_w == 0 || src_h == 0 {
        return (0, 0);
    }
    let by_w = (mw, (mw * src_h).div_ceil(src_w).max(1));
    if by_w.1 <= mh {
        by_w
    } else {
        ((mh * src_w / src_h).max(1), mh)
    }
}

// ------------------------------------------------------------ half blocks ---

/// U+2580 UPPER HALF BLOCK: foreground paints the top pixel, background the
/// bottom one. Two rows of pixels per row of cells, and no graphics support
/// needed anywhere.
pub fn halfblock(src: &Screen, cols: usize, rows: usize) -> String {
    let (w, h) = fit(src.w, src.h, cols, rows * 2);
    let img = scale(src, w, h);
    let mut out = String::with_capacity(w * h * 12);

    let mut y = 0;
    while y < h {
        let mut last: Option<((u8, u8, u8), (u8, u8, u8))> = None;
        for x in 0..w {
            let top = img.get(x, y);
            // An odd number of rows leaves the last cell's bottom half empty;
            // repeating the top pixel is less distracting than a black band.
            let bot = if y + 1 < h { img.get(x, y + 1) } else { top };
            // Only re-emit the colour when it changes. On a node showing a
            // scene with a flat background this is most of the frame.
            if last != Some((top, bot)) {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m",
                    top.0, top.1, top.2, bot.0, bot.1, bot.2
                ));
                last = Some((top, bot));
            }
            out.push('\u{2580}');
        }
        out.push_str("\x1b[0m");
        y += 2;
        if y < h {
            out.push_str("\r\n");
        }
    }
    out
}

// ------------------------------------------------------------------ sixel ---

/// A 6x6x6 colour cube: 216 registers, which every sixel terminal has room
/// for, and no quantiser to write.
///
/// A median-cut palette would look better and would mean carrying a quantiser
/// and its failure modes. The cube is uniform, stateless, and identical frame
/// to frame -- which matters more than it sounds like it should, because a
/// palette that shifts between frames makes a still screen appear to shimmer.
const LEVELS: usize = 6;

#[inline]
fn cube_index(r: u8, g: u8, b: u8) -> usize {
    let q = |v: u8| (v as usize * (LEVELS - 1) + 127) / 255;
    q(r) * LEVELS * LEVELS + q(g) * LEVELS + q(b)
}

/// Sixel wants colour components as percentages, 0..=100.
fn cube_colour(i: usize) -> (u8, u8, u8) {
    let r = i / (LEVELS * LEVELS);
    let g = (i / LEVELS) % LEVELS;
    let b = i % LEVELS;
    let pct = |v: usize| (v * 100 / (LEVELS - 1)) as u8;
    (pct(r), pct(g), pct(b))
}

pub fn sixel(src: &Screen, px_w: usize, px_h: usize) -> String {
    let (w, h) = fit(src.w, src.h, px_w, px_h);
    if w == 0 || h == 0 {
        return String::new();
    }
    let img = scale(src, w, h);

    // Quantise once. The band loop reads this many times and re-quantising
    // inside it was the whole cost of the first version.
    let mut idx = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = img.get(x, y);
            idx[y * w + x] = cube_index(r, g, b) as u16;
        }
    }

    let mut out = String::with_capacity(w * h / 2);
    // DCS, P1=0 (no aspect correction), P2=1 (0 means transparent; 1 means
    // background), P3=0, then 'q'.
    out.push_str("\x1bP0;1;0q");
    out.push_str(&format!("\"1;1;{};{}", w, h));

    // Only define the registers this image actually uses.
    let mut used = [false; LEVELS * LEVELS * LEVELS];
    for i in &idx {
        used[*i as usize] = true;
    }
    for (i, u) in used.iter().enumerate() {
        if *u {
            let (r, g, b) = cube_colour(i);
            out.push_str(&format!("#{};2;{};{};{}", i, r, g, b));
        }
    }

    // A band is six pixel rows. Within a band, each colour gets one pass.
    let mut band = 0;
    while band * 6 < h {
        let y0 = band * 6;
        let rows = (h - y0).min(6);

        // Which colours appear in this band at all.
        let mut here = [false; LEVELS * LEVELS * LEVELS];
        for dy in 0..rows {
            for x in 0..w {
                here[idx[(y0 + dy) * w + x] as usize] = true;
            }
        }

        let mut first = true;
        for (colour, present) in here.iter().enumerate() {
            if !*present {
                continue;
            }
            if !first {
                out.push('$'); // back to the left edge, same band
            }
            first = false;
            out.push_str(&format!("#{}", colour));

            // Each column becomes one character whose low six bits say which
            // of the six rows carry this colour.
            let mut run_char = 0u8;
            let mut run_len = 0usize;
            for x in 0..w {
                let mut bits = 0u8;
                for dy in 0..rows {
                    if idx[(y0 + dy) * w + x] as usize == colour {
                        bits |= 1 << dy;
                    }
                }
                let ch = 0x3F + bits;
                if run_len > 0 && ch == run_char {
                    run_len += 1;
                } else {
                    emit_run(&mut out, run_char, run_len);
                    run_char = ch;
                    run_len = 1;
                }
            }
            emit_run(&mut out, run_char, run_len);
        }

        band += 1;
        if band * 6 < h {
            out.push('-'); // next band
        }
    }

    out.push_str("\x1b\\"); // ST
    out
}

/// `!n<char>` is sixel's run length. Below four characters the escape costs
/// more than it saves.
fn emit_run(out: &mut String, ch: u8, len: usize) {
    if len == 0 {
        return;
    }
    if len > 3 {
        out.push_str(&format!("!{}", len));
        out.push(ch as char);
    } else {
        for _ in 0..len {
            out.push(ch as char);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, r: u8, g: u8, b: u8) -> Screen {
        let mut s = Screen::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                s.px[i] = r;
                s.px[i + 1] = g;
                s.px[i + 2] = b;
            }
        }
        s
    }

    #[test]
    fn fit_keeps_aspect_and_stays_inside() {
        // Wider than the box: width binds.
        let (w, h) = fit(1920, 1080, 200, 200);
        assert_eq!(w, 200);
        assert!(h <= 200, "{}x{} escaped the box", w, h);
        // Taller than the box: height binds.
        let (w, h) = fit(1080, 1920, 200, 100);
        assert!(w <= 200 && h <= 100, "{}x{} escaped the box", w, h);
    }

    #[test]
    fn fit_survives_a_zero() {
        assert_eq!(fit(0, 0, 10, 10), (0, 0));
    }

    #[test]
    fn the_colour_cube_round_trips_its_corners() {
        assert_eq!(cube_index(0, 0, 0), 0);
        assert_eq!(cube_index(255, 255, 255), 215);
        assert_eq!(cube_colour(0), (0, 0, 0));
        assert_eq!(cube_colour(215), (100, 100, 100));
    }

    #[test]
    fn every_cube_index_is_a_real_register() {
        for r in [0u8, 1, 40, 128, 200, 255] {
            for g in [0u8, 90, 255] {
                for b in [0u8, 17, 255] {
                    assert!(cube_index(r, g, b) < LEVELS * LEVELS * LEVELS);
                }
            }
        }
    }

    #[test]
    fn sixel_is_wrapped_in_dcs_and_st() {
        let s = sixel(&solid(12, 12, 255, 0, 0), 12, 12);
        assert!(s.starts_with("\x1bP"), "no DCS: {:?}", &s[..8.min(s.len())]);
        assert!(s.ends_with("\x1b\\"), "no ST");
        assert!(s.contains("\"1;1;12;12"), "no raster attributes: {}", s);
    }

    #[test]
    fn a_solid_frame_defines_one_register() {
        let s = sixel(&solid(12, 12, 255, 0, 0), 12, 12);
        let defs = s.matches(";2;").count();
        assert_eq!(defs, 1, "a solid red frame should need one colour: {}", s);
    }

    #[test]
    fn a_solid_frame_run_length_encodes() {
        // 60 wide, one colour: the row should collapse to a run, not 60 chars.
        let s = sixel(&solid(60, 6, 0, 0, 255), 60, 6);
        assert!(s.contains("!60"), "no run of 60 in {}", s);
    }

    #[test]
    fn sixel_survives_a_zero_sized_target() {
        assert_eq!(sixel(&solid(10, 10, 1, 2, 3), 0, 0), "");
    }

    #[test]
    fn halfblock_emits_one_cell_per_column_and_resets() {
        let out = halfblock(&solid(4, 4, 10, 20, 30), 4, 2);
        assert_eq!(out.matches('\u{2580}').count(), 4 * 2);
        assert!(out.contains("\x1b[0m"), "a row must not leak its colour");
        assert!(out.contains("38;2;10;20;30"), "true colour foreground missing");
    }

    #[test]
    fn halfblock_repeats_a_colour_only_when_it_changes() {
        // A solid frame: one colour escape for the whole row, not one per cell.
        let out = halfblock(&solid(20, 2, 7, 7, 7), 20, 1);
        assert_eq!(out.matches("38;2;7;7;7").count(), 1);
    }

    #[test]
    fn halfblock_handles_an_odd_pixel_height() {
        // 3 pixel rows into 2 cell rows: the last cell has no bottom pixel.
        let s = solid(4, 3, 1, 2, 3);
        let out = halfblock(&s, 4, 2);
        assert!(out.contains('\u{2580}'));
    }

    #[test]
    fn scaling_down_picks_real_pixels() {
        let mut s = Screen::new(4, 4);
        // Top-left quadrant red, rest black.
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 4 + x) * 3;
                s.px[i] = 255;
            }
        }
        let small = scale(&s, 2, 2);
        assert_eq!(small.get(0, 0), (255, 0, 0));
        assert_eq!(small.get(1, 1), (0, 0, 0));
    }
}
