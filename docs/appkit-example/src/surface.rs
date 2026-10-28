//! The seam between the drawing and whatever shows it.
//!
//! The drawing (`draw.rs`) knows nothing about windows: it writes pixels into
//! a buffer. A platform is anything that can hand over a buffer, take it back
//! to show, and say what the person did. The Mac window is one (`mac.rs`); a
//! file is another (`Offscreen`), which is how a frame is looked at with no
//! window at all.

/// What the person did, in the program's own words, not AppKit's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    /// The pointer, in canvas pixels, top left at (0, 0).
    Moved { x: i32, y: i32 },
    Clicked { x: i32, y: i32 },
    /// A typed character; `cmd` when Command was held.
    Key { ch: char, cmd: bool },
    Escape,
    Resized,
    Closed,
}

pub trait Surface {
    /// `width * height` words of `0x00RRGGBB`, native-endian.
    fn pixels(&mut self) -> &mut [u32];
    fn size(&self) -> (usize, usize);
    /// Everything that happened since the last call. Never blocks.
    fn poll(&mut self) -> Vec<Input>;
    /// Adopt the size the platform asked for.
    fn apply_resize(&mut self);
    /// Hand the pixels over to be shown.
    fn present(&mut self);
}

/// A surface that is only a buffer: `--frame PATH` draws into it and writes
/// a PPM, so every picture can be made without a screen.
pub struct Offscreen {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

impl Offscreen {
    pub fn new(w: usize, h: usize) -> Offscreen {
        Offscreen { w, h, px: vec![0; w * h] }
    }

    pub fn write_ppm(&self, path: &str) -> std::io::Result<()> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        for &p in &self.px {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
        std::fs::write(path, out)
    }
}

impl Surface for Offscreen {
    fn pixels(&mut self) -> &mut [u32] {
        &mut self.px
    }
    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn poll(&mut self) -> Vec<Input> {
        Vec::new()
    }
    fn apply_resize(&mut self) {}
    fn present(&mut self) {}
}
