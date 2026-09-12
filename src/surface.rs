//! The seam between the drawing and whatever is willing to show it.
//!
//! `draw.rs` already had no idea what a window is -- it takes a `&mut [u8]`, a
//! width and a height, and writes pixels. That was half luck: phase 2 wrote a
//! software canvas because a Zero 2 should not carry an EGL stack, and the
//! consequence is that a second platform is a new file rather than a new
//! program.
//!
//! Four things are platform-specific: getting a rectangle of memory somebody
//! will put on a screen, being told it changed size, being told the user closed
//! it, and being told what the user did. `wl::Window` already does the first
//! three. The fourth is new, and it is new on both platforms at once.
//!
//! THE TRAIT IS NOT HERE FOR A THIRD PLATFORM. There will not be one. It is
//! here so the museum interface can be written once against a thing that
//! produces `Input` and accepts pixels -- and so that `--frame`, which has no
//! platform at all, stays a first-class way to look at the drawing.

/// Which pointer button. Three, because that is what a pointer has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// The modifier keys that change what a click or a keystroke means.
///
/// Four, as a bitfield. `logo` is Command on a Mac and Super on a node, and
/// the interface treats them as the same key because the gesture -- "add this
/// one to the selection" -- is the same gesture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Mods {
    /// The "extend the selection by one" chord, spelled the way each platform
    /// spells it. Asking for this rather than for a specific key is what keeps
    /// `lab.rs` from having a `#[cfg(target_os)]` in the middle of a click.
    pub fn toggling(&self) -> bool {
        self.ctrl || self.logo
    }
}

/// A named key, or a character.
///
/// DELIBERATELY NOT X11 KEYSYM NUMBERING. Nothing here interoperates with an X
/// server, so this is a small enum of the keys the interface actually binds,
/// and `Char` for everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Return,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    /// F1..F12, carried as its number rather than twelve variants.
    Func(u8),
    /// A key this build has no name for. Still carries its scancode, so
    /// Control can forward it even when the interface cannot bind it.
    Unknown,
}

/// Everything that has happened since the last time anyone asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Motion { x: i32, y: i32 },
    Button { x: i32, y: i32, button: Button, down: bool },
    Scroll { dx: i32, dy: i32 },

    /// A physical key.
    ///
    /// `scancode` is PC/AT set 1, which is what RDP's fastpath input events
    /// carry. `sym` is what the interface binds. BOTH TRAVEL, AND THAT IS NOT
    /// REDUNDANCY: Terminal wants characters because a shell is fed UTF-8,
    /// Control wants scancodes because the far end does its own keymapping.
    /// The same keypress is two facts and an interface that kept one of them
    /// would have to reconstruct the other badly.
    Key { scancode: u16, sym: Sym, down: bool, mods: Mods },

    /// What that key means as text, when it means any. Already composed --
    /// on a Mac this is `NSEvent.characters` and carries the whole input
    /// method stack for free.
    Text(String),

    Resized { w: usize, h: usize },
    Closed,
    /// The platform thinks now is a good time to draw.
    Frame,
}

/// A rectangle of pixels somebody else is willing to put on a screen.
pub trait Surface {
    /// `width * height` words of `0x00RRGGBB`, native-endian. The canvas.
    fn pixels(&mut self) -> &mut [u8];
    fn size(&self) -> (usize, usize);

    /// Everything the platform has said since the last call. Never blocks.
    fn poll(&mut self) -> Result<Vec<Input>, String>;

    /// Adopt a size the platform asked for, reallocating the buffer.
    fn apply_resize(&mut self) -> Result<(), String>;

    /// Hand the pixels over.
    fn present(&mut self) -> Result<(), String>;

    fn closed(&self) -> bool;

    /// True while the platform is still reading the last buffer. Painting into
    /// it is the classic tear.
    fn busy(&self) -> bool {
        false
    }
}

/// A surface that is a plain buffer and no platform at all.
///
/// This is what `--frame` renders through, and it is the reason every screen
/// in `docs/console.md` can be regenerated into a file, diffed, and pasted
/// into a review. A console whose screens can only be seen by standing in
/// front of one is a console nobody reviews.
pub struct Offscreen {
    px: Vec<u8>,
    w: usize,
    h: usize,
    queued: Vec<Input>,
}

impl Offscreen {
    pub fn new(w: usize, h: usize) -> Offscreen {
        Offscreen { px: vec![0; w * h * 4], w, h, queued: Vec::new() }
    }

    /// Hand it a script of events, which is how `ui.rs` is tested without a
    /// compositor: a frame is a pure function of canvas, theme, input and
    /// state, so a button press is three lines.
    pub fn feed(&mut self, events: Vec<Input>) {
        self.queued = events;
    }

    /// The canvas as 24-bit RGB rows, which is what a PPM wants.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        out.reserve(self.w * self.h * 3);
        for word in self.px.chunks_exact(4) {
            let px = u32::from_ne_bytes([word[0], word[1], word[2], word[3]]);
            out.push((px >> 16) as u8);
            out.push((px >> 8) as u8);
            out.push(px as u8);
        }
        out
    }
}

impl Surface for Offscreen {
    fn pixels(&mut self) -> &mut [u8] {
        &mut self.px
    }
    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn poll(&mut self) -> Result<Vec<Input>, String> {
        Ok(std::mem::take(&mut self.queued))
    }
    fn apply_resize(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn present(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn closed(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toggling_chord_is_the_same_gesture_on_both_platforms() {
        let cmd = Mods { logo: true, ..Default::default() };
        let ctrl = Mods { ctrl: true, ..Default::default() };
        assert!(cmd.toggling(), "Command did not toggle");
        assert!(ctrl.toggling(), "Control did not toggle");
        assert!(!Mods::default().toggling());
        // Shift is the range gesture, not the toggle gesture.
        assert!(!Mods { shift: true, ..Default::default() }.toggling());
    }

    #[test]
    fn an_offscreen_surface_hands_back_what_it_was_fed_exactly_once() {
        let mut s = Offscreen::new(4, 2);
        s.feed(vec![Input::Closed]);
        assert_eq!(s.poll().unwrap(), vec![Input::Closed]);
        assert!(s.poll().unwrap().is_empty(), "an event was delivered twice");
    }

    #[test]
    fn a_ppm_is_the_canvas_in_the_order_a_ppm_wants() {
        let mut s = Offscreen::new(2, 1);
        {
            let px = s.pixels();
            // 0x00RRGGBB, native-endian, which is what the canvas holds.
            px[0..4].copy_from_slice(&0x00ff8000u32.to_ne_bytes());
            px[4..8].copy_from_slice(&0x00003366u32.to_ne_bytes());
        }
        let ppm = s.to_ppm();
        let head = b"P6\n2 1\n255\n";
        assert_eq!(&ppm[..head.len()], head);
        assert_eq!(&ppm[head.len()..], &[0xff, 0x80, 0x00, 0x00, 0x33, 0x66]);
    }
}

/// Blow a logical canvas up into a device one: every pixel becomes an `n` by
/// `n` block.
///
/// WHY THIS EXISTS. The interface is drawn in 8x13 and 10x20 bitmap glyphs,
/// which are the right size on a 96-dpi panel and are physically tiny on a
/// Retina Mac or a 4K screen — the canvas is allocated in DEVICE pixels, so
/// twice the density means half the size. Scaling the glyphs is not an option:
/// they are bitmaps, and a bitmap font at 1.5x is a smear.
///
/// So the whole interface is drawn at a logical size and each pixel is
/// repeated. NEAREST NEIGHBOUR AND INTEGER FACTORS ONLY, which for a wall of
/// flat rectangles and pixel letters is not a compromise -- it is exactly what
/// the drawing wants, and every edge stays where it was put.
pub fn expand(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize, n: usize) {
    if n <= 1 {
        let bytes = (sw * sh * 4).min(dst.len()).min(src.len());
        dst[..bytes].copy_from_slice(&src[..bytes]);
        return;
    }
    for y in 0..sh {
        let dy0 = y * n;
        if dy0 >= dh {
            break;
        }
        // One source row, expanded once into the first of its output rows,
        // then copied to the rest -- a memcpy per row instead of a multiply
        // per pixel.
        let first = dy0 * dw * 4;
        for x in 0..sw {
            let s = (y * sw + x) * 4;
            if s + 4 > src.len() {
                break;
            }
            let p = &src[s..s + 4];
            for k in 0..n {
                let d = first + (x * n + k) * 4;
                if d + 4 > dst.len() {
                    break;
                }
                dst[d..d + 4].copy_from_slice(p);
            }
        }
        let row = (dw * 4).min(dst.len() - first);
        for k in 1..n {
            let dy = dy0 + k;
            if dy >= dh {
                break;
            }
            let to = dy * dw * 4;
            if to + row > dst.len() {
                break;
            }
            let (a, b) = dst.split_at_mut(to);
            b[..row].copy_from_slice(&a[first..first + row]);
        }
    }
}

#[cfg(test)]
mod expand_tests {
    use super::*;

    #[test]
    fn every_pixel_becomes_a_block() {
        // Two by two, doubled: each pixel fills a 2x2 square and nothing
        // bleeds into its neighbour.
        let src: Vec<u8> = vec![
            1, 1, 1, 255, 2, 2, 2, 255, // row 0
            3, 3, 3, 255, 4, 4, 4, 255, // row 1
        ];
        let mut dst = vec![0u8; 4 * 4 * 4];
        expand(&src, 2, 2, &mut dst, 4, 4, 2);
        let at = |x: usize, y: usize| dst[(y * 4 + x) * 4];
        assert_eq!([at(0, 0), at(1, 0), at(0, 1), at(1, 1)], [1, 1, 1, 1]);
        assert_eq!([at(2, 0), at(3, 0), at(2, 1), at(3, 1)], [2, 2, 2, 2]);
        assert_eq!([at(0, 2), at(1, 3)], [3, 3]);
        assert_eq!([at(2, 2), at(3, 3)], [4, 4]);
    }

    #[test]
    fn a_scale_of_one_is_a_copy() {
        let src: Vec<u8> = (0..16).collect();
        let mut dst = vec![0u8; 16];
        expand(&src, 2, 2, &mut dst, 2, 2, 1);
        assert_eq!(dst, src);
    }

    #[test]
    fn a_destination_that_does_not_divide_evenly_is_not_a_panic() {
        // The window is whatever the operator dragged it to, so the device
        // buffer is almost never an exact multiple of the logical one.
        let src: Vec<u8> = vec![9u8; 3 * 3 * 4];
        let mut dst = vec![0u8; 7 * 7 * 4];
        expand(&src, 3, 3, &mut dst, 7, 7, 2);
        assert_eq!(dst[0], 9);
        // The last column and row of the destination are simply not reached,
        // which is a border rather than a fault.
        assert_eq!(dst[(6 * 7 + 6) * 4], 0);
    }
}
