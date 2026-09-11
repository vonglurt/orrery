//! A software canvas: the only drawing this program does.
//!
//! There is no renderer underneath this and no GPU. A Wayland shared-memory
//! surface is a flat array of pixels, so everything the console draws is
//! written here, one word at a time, and `wl.rs` hands the result to the
//! compositor. On a Zero 2 that is the right trade twice over: a wall of flat
//! rectangles and small text costs almost nothing to rasterise, and the EGL
//! stack it avoids is both a dependency and a thing that fails differently on
//! every board.
//!
//! EVERY PRIMITIVE CLIPS, and takes signed coordinates so that it can. A panel
//! half off the left edge of a narrow window is an ordinary thing for a layout
//! to ask for; the alternative to clipping here is every caller checking, and
//! one of them forgetting.
//!
//! THE PALETTE IS THE PAGE'S. `assets/wall.html` already carries a designed
//! theme, and `docs/THEME.md` in the fleet repo is where it came from. The
//! native console uses the same values rather than inventing a second set, so
//! the two faces of the same program look like each other.

use crate::font::Font;

/// A colour as `0x00RRGGBB`, which is what XRGB8888 holds.
pub type Rgb = u32;

/// The wall's colours, both ways round.
///
/// A gallery screen in a lit room and an operator's screen at night are the
/// same two cases a browser calls light and dark, so the values are the two
/// blocks at the top of `wall.html` and nothing new.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub base: Rgb,
    pub panel: Rgb,
    pub tile: Rgb,
    pub ink: Rgb,
    pub dim: Rgb,
    pub line: Rgb,
    pub accent: Rgb,
    pub up: Rgb,
    pub warn: Rgb,
    pub alarm: Rgb,
}

pub const LIGHT: Theme = Theme {
    base: 0xfce2ab,
    panel: 0xf7d79a,
    tile: 0xfff2d4,
    ink: 0x121212,
    dim: 0x6b5836,
    line: 0xc4a870,
    accent: 0x87704f,
    up: 0x4a7c4e,
    warn: 0xb06a24,
    alarm: 0xff723e,
};

pub const DARK: Theme = Theme {
    base: 0x181818,
    panel: 0x121212,
    tile: 0x1f1f1f,
    ink: 0xd0daed,
    dim: 0x87704f,
    line: 0x2a2a2a,
    accent: 0xfccf8a,
    up: 0x8fbf7a,
    warn: 0xfccf8a,
    alarm: 0xff723e,
};

/// Mix `a` toward `b`. `t` is 0..=255.
///
/// Used for the one thing flat colour cannot do: a temperature bar that has to
/// read as a quantity rather than a state.
pub fn mix(a: Rgb, b: Rgb, t: u8) -> Rgb {
    // SIGNED THROUGHOUT, because the delta is often negative and casting a
    // negative difference to u32 turns it into four billion. The pair this
    // exists for -- `up` toward `alarm` -- has a descending green channel, so
    // the unsigned version was wrong on the one gradient the wall draws.
    let f = |shift: u32| -> u32 {
        let x = ((a >> shift) & 0xff) as i32;
        let y = ((b >> shift) & 0xff) as i32;
        (x + (y - x) * t as i32 / 255).clamp(0, 255) as u32
    };
    (f(16) << 16) | (f(8) << 8) | f(0)
}

/// Somewhere to draw: a borrowed pixel buffer and its shape.
pub struct Canvas<'a> {
    px: &'a mut [u8],
    pub w: usize,
    pub h: usize,
}

impl<'a> Canvas<'a> {
    /// `px` must be at least `w * h * 4` bytes. The surplus a Wayland pool
    /// keeps after a window shrinks is allowed and ignored.
    pub fn new(px: &'a mut [u8], w: usize, h: usize) -> Canvas<'a> {
        assert!(px.len() >= w * h * 4, "the buffer is smaller than {}x{}", w, h);
        Canvas { px, w, h }
    }

    pub fn clear(&mut self, c: Rgb) {
        let word = c.to_ne_bytes();
        let end = self.w * self.h * 4;
        for chunk in self.px[..end].chunks_exact_mut(4) {
            chunk.copy_from_slice(&word);
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.px[i..i + 4].copy_from_slice(&c.to_ne_bytes());
    }

    /// A filled rectangle, clipped. Negative sizes draw nothing rather than
    /// wrapping round, which is what an unchecked `as usize` would do.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        if w <= 0 || h <= 0 {
            return;
        }
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w).max(0) as usize).min(self.w);
        let y1 = ((y + h).max(0) as usize).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let word = c.to_ne_bytes();
        for row in y0..y1 {
            let start = (row * self.w + x0) * 4;
            let end = (row * self.w + x1) * 4;
            for chunk in self.px[start..end].chunks_exact_mut(4) {
                chunk.copy_from_slice(&word);
            }
        }
    }

    /// A one-pixel outline, drawn inside the given rectangle.
    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.rect(x, y, w, 1, c);
        self.rect(x, y + h - 1, w, 1, c);
        self.rect(x, y, 1, h, c);
        self.rect(x + w - 1, y, 1, h, c);
    }

    pub fn hline(&mut self, x: i32, y: i32, len: i32, c: Rgb) {
        self.rect(x, y, len, 1, c);
    }

    pub fn vline(&mut self, x: i32, y: i32, len: i32, c: Rgb) {
        self.rect(x, y, 1, len, c);
    }

    /// A filled circle, by the midpoint test rather than by trigonometry.
    ///
    /// This is the node's status light, drawn at six or seven pixels across,
    /// where anti-aliasing would make it look smudged rather than round.
    pub fn disc(&mut self, cx: i32, cy: i32, r: i32, c: Rgb) {
        if r <= 0 {
            return;
        }
        // The half-pixel bias makes an even diameter come out symmetrical.
        let limit = (r * r * 4 + r * 2) / 4;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= limit {
                    self.set(cx + dx, cy + dy, c);
                }
            }
        }
    }

    /// The same circle, outline only: a node that is declared but not there.
    pub fn ring(&mut self, cx: i32, cy: i32, r: i32, c: Rgb) {
        if r <= 0 {
            return;
        }
        let outer = (r * r * 4 + r * 2) / 4;
        let inner = ((r - 1) * (r - 1) * 4 + (r - 1) * 2) / 4;
        for dy in -r..=r {
            for dx in -r..=r {
                let d = dx * dx + dy * dy;
                if d <= outer && d > inner {
                    self.set(cx + dx, cy + dy, c);
                }
            }
        }
    }

    /// Draw `s` with its cell's top-left corner at `(x, y)`.
    ///
    /// Returns the x the next character would start at, so runs of differently
    /// coloured text compose without the caller measuring anything.
    pub fn text(&mut self, x: i32, y: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        let mut at = x;
        for ch in s.chars() {
            // Nothing to draw, and skipping the inner loop matters: a wall is
            // mostly spaces and padding.
            if ch != ' ' {
                self.glyph(at, y, ch, font, c);
            }
            at += font.advance() as i32;
        }
        at
    }

    fn glyph(&mut self, x: i32, y: i32, ch: char, font: &Font, c: Rgb) {
        // Wholly off-canvas: the common case when a layout runs past an edge.
        if x + (font.width as i32) <= 0
            || y + (font.height as i32) <= 0
            || x >= self.w as i32
            || y >= self.h as i32
        {
            return;
        }
        let rows = font.glyph(ch);
        for (dy, bits) in rows.iter().enumerate() {
            if *bits == 0 {
                continue;
            }
            for dx in 0..font.width {
                if (bits >> dx) & 1 == 1 {
                    self.set(x + dx as i32, y + dy as i32, c);
                }
            }
        }
    }

    /// Draw `s`, but never past `max_w` pixels, ending in an ellipsis when it
    /// has to cut.
    ///
    /// A node id or a scene name that runs into the next column is the single
    /// most common way a fixed layout goes wrong, and the answer is not to
    /// widen the column -- it is to say plainly that there is more.
    pub fn text_in(&mut self, x: i32, y: i32, max_w: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        let cw = font.advance() as i32;
        if max_w < cw {
            return x;
        }
        let fits = (max_w / cw) as usize;
        let count = s.chars().count();
        if count <= fits {
            return self.text(x, y, s, font, c);
        }
        // One cell goes to the marker, so the text keeps `fits - 1`.
        let keep: String = s.chars().take(fits.saturating_sub(1)).collect();
        let at = self.text(x, y, &keep, font, c);
        self.text(at, y, "\u{2026}", font, c)
    }

    /// A right-aligned run, which is what every number in the wall wants.
    pub fn text_right(&mut self, right: i32, y: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        let x = right - font.measure(s) as i32;
        self.text(x, y, s, font, c);
        x
    }

    /// Read one pixel back. For tests, and for nothing else -- a canvas that
    /// is sampled during drawing is a canvas whose order of operations has
    /// become load-bearing.
    pub fn get(&self, x: usize, y: usize) -> Rgb {
        let i = (y * self.w + x) * 4;
        let word = u32::from_ne_bytes([
            self.px[i],
            self.px[i + 1],
            self.px[i + 2],
            self.px[i + 3],
        ]);
        word & 0x00ff_ffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{F10X20, F8X13};

    fn canvas(w: usize, h: usize) -> (Vec<u8>, usize, usize) {
        (vec![0u8; w * h * 4], w, h)
    }

    #[test]
    fn clear_paints_every_pixel_and_no_more() {
        let (mut px, w, h) = canvas(4, 3);
        px.push(0xaa); // a byte past the end, which must survive
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0x112233);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(c.get(x, y), 0x112233, "at {},{}", x, y);
            }
        }
        assert_eq!(px[w * h * 4], 0xaa, "clear ran past the picture");
    }

    #[test]
    fn a_rect_fills_exactly_its_own_area() {
        let (mut px, w, h) = canvas(8, 8);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.rect(2, 3, 3, 2, 0xff0000);
        for y in 0..h {
            for x in 0..w {
                let inside = (2..5).contains(&x) && (3..5).contains(&y);
                assert_eq!(
                    c.get(x, y),
                    if inside { 0xff0000 } else { 0 },
                    "at {},{}",
                    x,
                    y
                );
            }
        }
    }

    #[test]
    fn a_rect_hanging_off_the_top_left_is_clipped_not_wrapped() {
        let (mut px, w, h) = canvas(8, 8);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.rect(-3, -2, 5, 4, 0x00ff00);
        assert_eq!(c.get(0, 0), 0x00ff00);
        assert_eq!(c.get(1, 1), 0x00ff00);
        // It reached x=1 and y=1 and stopped.
        assert_eq!(c.get(2, 0), 0);
        assert_eq!(c.get(0, 2), 0);
        // Nothing wrapped round to the far edge.
        assert_eq!(c.get(7, 7), 0);
    }

    #[test]
    fn a_rect_hanging_off_the_bottom_right_is_clipped() {
        let (mut px, w, h) = canvas(8, 8);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.rect(6, 6, 10, 10, 0x0000ff);
        assert_eq!(c.get(7, 7), 0x0000ff);
        assert_eq!(c.get(5, 5), 0);
    }

    #[test]
    fn a_rect_entirely_outside_draws_nothing() {
        let (mut px, w, h) = canvas(4, 4);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0x010101);
        c.rect(100, 100, 5, 5, 0xffffff);
        c.rect(-50, -50, 5, 5, 0xffffff);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(c.get(x, y), 0x010101);
            }
        }
    }

    #[test]
    fn a_rect_with_no_size_draws_nothing() {
        let (mut px, w, h) = canvas(4, 4);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.rect(1, 1, 0, 5, 0xffffff);
        c.rect(1, 1, 5, -3, 0xffffff);
        assert_eq!(c.get(1, 1), 0);
    }

    #[test]
    fn a_frame_is_hollow() {
        let (mut px, w, h) = canvas(6, 6);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.frame(1, 1, 4, 4, 0xabcdef);
        assert_eq!(c.get(1, 1), 0xabcdef, "corner");
        assert_eq!(c.get(4, 4), 0xabcdef, "far corner");
        assert_eq!(c.get(3, 1), 0xabcdef, "top edge");
        assert_eq!(c.get(2, 2), 0, "the middle stays empty");
    }

    #[test]
    fn a_disc_is_round_and_symmetrical() {
        let (mut px, w, h) = canvas(21, 21);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.disc(10, 10, 5, 0xffffff);
        assert_eq!(c.get(10, 10), 0xffffff, "the centre");
        // Mirror symmetry in both axes is the cheap test that catches an
        // off-by-one in the midpoint bias.
        for dy in 0..=5i32 {
            for dx in 0..=5i32 {
                let a = c.get((10 + dx) as usize, (10 + dy) as usize);
                let b = c.get((10 - dx) as usize, (10 + dy) as usize);
                let d = c.get((10 + dx) as usize, (10 - dy) as usize);
                assert_eq!(a, b, "not mirrored in x at {},{}", dx, dy);
                assert_eq!(a, d, "not mirrored in y at {},{}", dx, dy);
            }
        }
        // And it really is a disc, not a square.
        assert_eq!(c.get(15, 15), 0, "the corner of the bounding box is empty");
    }

    #[test]
    fn a_ring_is_hollow() {
        let (mut px, w, h) = canvas(21, 21);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.ring(10, 10, 6, 0xffffff);
        assert_eq!(c.get(10, 10), 0, "a ring has no middle");
        assert_eq!(c.get(16, 10), 0xffffff, "but it has an edge");
    }

    #[test]
    fn a_disc_clipped_by_the_edge_does_not_wrap() {
        let (mut px, w, h) = canvas(10, 10);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.disc(0, 0, 4, 0xffffff);
        assert_eq!(c.get(0, 0), 0xffffff);
        assert_eq!(c.get(9, 9), 0, "nothing appeared at the opposite corner");
    }

    // --- text ---------------------------------------------------------------

    #[test]
    fn text_advances_by_the_cell_width() {
        let (mut px, w, h) = canvas(200, 30);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        let end = c.text(10, 2, "museum-01", &F8X13, 0xffffff);
        assert_eq!(end, 10 + 9 * 8, "nine characters at eight pixels");
        assert_eq!(F8X13.measure("museum-01"), 72);
    }

    #[test]
    fn a_glyph_puts_ink_where_the_font_says() {
        let (mut px, w, h) = canvas(16, 16);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        c.text(0, 0, "M", &F8X13, 0xffffff);
        // From the font: 'M' has its stems in columns 0 and 6, on row 2.
        assert_eq!(c.get(0, 2), 0xffffff, "left stem");
        assert_eq!(c.get(6, 2), 0xffffff, "right stem");
        assert_eq!(c.get(3, 2), 0, "and nothing between them on that row");
    }

    #[test]
    fn a_space_leaves_the_background_alone() {
        let (mut px, w, h) = canvas(32, 16);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0x123456);
        c.text(0, 0, " ", &F8X13, 0xffffff);
        for x in 0..8 {
            for y in 0..13 {
                assert_eq!(c.get(x, y), 0x123456, "a space drew ink at {},{}", x, y);
            }
        }
    }

    #[test]
    fn text_off_the_left_edge_is_clipped_and_still_advances() {
        let (mut px, w, h) = canvas(16, 16);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        // First two characters are entirely off-canvas.
        let end = c.text(-16, 0, "MMM", &F8X13, 0xffffff);
        assert_eq!(end, -16 + 24, "advance does not depend on visibility");
        assert_eq!(c.get(0, 2), 0xffffff, "the third one landed");
    }

    #[test]
    fn a_character_outside_the_baked_range_is_blank_rather_than_a_panic() {
        let (mut px, w, h) = canvas(32, 24);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        // Well past Latin-1. A node's own output reaches this console, so it
        // must survive whatever a scene name turns out to contain.
        c.text(0, 0, "\u{4e2d}\u{6587}", &F8X13, 0xffffff);
        for x in 0..16 {
            for y in 0..13 {
                assert_eq!(c.get(x, y), 0, "unknown glyph drew something");
            }
        }
    }

    #[test]
    fn the_two_latin1_marks_the_wall_needs_are_present() {
        // The degree sign and the middle dot are the only characters past
        // ASCII the console actually prints, and a font missing them would
        // show up as gaps nobody traced back to here.
        for ch in ['\u{b0}', '\u{b7}'] {
            let rows = F8X13.glyph(ch);
            assert!(rows.iter().any(|r| *r != 0), "{:?} is blank in 8x13", ch);
            let rows = F10X20.glyph(ch);
            assert!(rows.iter().any(|r| *r != 0), "{:?} is blank in 10x20", ch);
        }
    }

    #[test]
    fn the_truncation_marker_is_actually_in_the_font() {
        // The bug this replaces: `text_in` cut a string and drew U+2026, which
        // was outside the baked range, so truncation was silent -- the worst
        // of both outcomes. It is in EXTRAS now, and this is what keeps it so.
        for f in [&F8X13, &F10X20] {
            let rows = f.glyph('\u{2026}');
            assert!(rows.iter().any(|r| *r != 0), "the ellipsis is blank");
        }
    }

    #[test]
    fn every_extra_baked_glyph_has_ink() {
        use crate::font::EXTRAS;
        for code in EXTRAS {
            let ch = char::from_u32(code).unwrap();
            for f in [&F8X13, &F10X20] {
                assert!(
                    f.glyph(ch).iter().any(|r| *r != 0),
                    "U+{:04X} is blank, so it is baked in name only",
                    code
                );
            }
        }
    }

    #[test]
    fn both_faces_have_the_shape_they_claim() {
        assert_eq!((F8X13.width, F8X13.height), (8, 13));
        assert_eq!((F10X20.width, F10X20.height), (10, 20));
        // A glyph slice is the face's height, not the storage array's.
        assert_eq!(F8X13.glyph('A').len(), 13);
        assert_eq!(F10X20.glyph('A').len(), 20);
    }

    #[test]
    fn text_in_truncates_with_a_marker_rather_than_overflowing() {
        let (mut px, w, h) = canvas(200, 20);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        // Room for five cells: four characters and the ellipsis.
        c.text_in(0, 0, 40, "museum-01", &F8X13, 0xffffff);
        // Column 40 onward must be untouched.
        for x in 40..200 {
            for y in 0..13 {
                assert_eq!(c.get(x, y), 0, "text_in ran past its width at {}", x);
            }
        }
    }

    #[test]
    fn text_in_leaves_a_string_that_fits_alone() {
        let (mut px, w, h) = canvas(200, 20);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        let end = c.text_in(0, 0, 200, "ok", &F8X13, 0xffffff);
        assert_eq!(end, 16, "no ellipsis when it fits");
    }

    #[test]
    fn text_in_with_no_room_at_all_draws_nothing() {
        let (mut px, w, h) = canvas(40, 20);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        let end = c.text_in(0, 0, 3, "museum", &F8X13, 0xffffff);
        assert_eq!(end, 0);
        assert_eq!(c.get(0, 2), 0);
    }

    #[test]
    fn text_right_ends_where_it_was_told_to() {
        let (mut px, w, h) = canvas(200, 20);
        let mut c = Canvas::new(&mut px, w, h);
        c.clear(0);
        let start = c.text_right(100, 0, "45\u{b0}C", &F8X13, 0xffffff);
        assert_eq!(start, 100 - 4 * 8);
    }

    // --- colour -------------------------------------------------------------

    #[test]
    fn mix_reaches_both_ends_and_the_middle() {
        assert_eq!(mix(0x000000, 0xffffff, 0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 255), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 128), 0x808080);
    }

    #[test]
    fn mix_handles_a_channel_that_goes_down() {
        // The regression: `up` (0x4a7c4e) toward `alarm` (0xff723e) has green
        // falling from 0x7c to 0x72, and an unsigned subtraction made that
        // overflow. This is the fleet's actual temperature gradient.
        let hot = mix(0x4a7c4e, 0xff723e, 255);
        assert_eq!(hot, 0xff723e);
        let mid = mix(0x4a7c4e, 0xff723e, 128);
        assert!((mid >> 8) & 0xff <= 0x7c, "green should fall, got {:#08x}", mid);
        assert!((mid >> 8) & 0xff >= 0x72, "and not below the target");
    }

    #[test]
    fn mix_keeps_channels_apart() {
        // If the channels bled into each other this would not stay pure red.
        assert_eq!(mix(0xff0000, 0xff0000, 128), 0xff0000);
        assert_eq!(mix(0xff0000, 0x00ff00, 255), 0x00ff00);
    }

    #[test]
    fn the_two_themes_are_actually_different_and_both_readable() {
        assert_ne!(LIGHT.base, DARK.base);
        // Ink on base must not be the same colour in either theme, which is
        // the cheapest possible guard against a palette edit that erases text.
        assert_ne!(LIGHT.ink, LIGHT.base);
        assert_ne!(DARK.ink, DARK.base);
    }
}
