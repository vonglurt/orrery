//! A window on a Mac, by hand.
//!
//! WHY THIS EXISTS AT ALL. `answers.txt`, the Makefile, `copal-prep.sh`, the
//! certificate authority, the token ledger and the card reader are all on the
//! operator's Mac. A Wayland-only console could show a fleet and could not
//! make one -- so the media pane of `docs/media.md` is the reason this file is
//! here, and the interface opening on the machine it is written on is the
//! reason it comes before the rest of the work rather than after.
//!
//! ZERO CRATES, AND THE SECOND HONEST EXCEPTION. `sys.rs` declares nine
//! functions from the system libc with `unsafe extern "C"` because `std` has
//! no `memfd_create`. This declares the Objective-C runtime's three entry
//! points and a handful of Core Graphics functions for the same reason and
//! with the same argument: `AppKit`, `Foundation`, `CoreGraphics` and
//! `libobjc` are on every Mac, nothing is fetched, nothing is vendored, and
//! `Cargo.toml`'s `[dependencies]` stays empty.
//!
//! NO OBJECTIVE-C CLASSES ARE CREATED AT RUNTIME, AND THAT IS THE DESIGN.
//! The usual way to do this is `objc_allocateClassPair`, `class_addMethod`, a
//! `drawRect:` implementation and a window delegate -- a large amount of very
//! fiddly unsafe code whose entire purpose is to let AppKit call back INTO
//! Rust. This program does not need callbacks. It has a loop, and everything
//! it needs is either a message send or a thing it can poll:
//!
//!     a window          NSWindow initWithContentRect:styleMask:backing:defer:
//!     pixels on it      an NSImageView content view, given a new image
//!     the pixels        CGDataProviderCreateWithData -> CGImageCreate
//!     events            nextEventMatchingMask: with a distant-past date
//!     "it was closed"   [window isVisible] went false
//!     "it was resized"  the content view's frame changed since last frame
//!
//! The last two are POLLED rather than delivered, which is slightly crude and
//! entirely sufficient at six frames a second.

#![cfg(target_os = "macos")]

use std::ffi::{c_char, c_void, CStr, CString};

use crate::surface::{Button, Input, Mods, Sym, Surface};

// ----------------------------------------------------------- the runtime ---

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "AppKit", kind = "framework")]
#[link(name = "Foundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;

    /// DECLARED WITH NO ARGUMENTS ON PURPOSE, AND NEVER CALLED THIS WAY.
    ///
    /// `objc_msgSend` is declared variadic in the headers and is not a
    /// variadic function: calling it through one prototype for every call site
    /// corrupts arguments in a way that depends on their types. Every call
    /// below transmutes it to the exact signature of that call. On arm64 there
    /// is no `_stret` and no `_fpret` to worry about, and an `NSRect` -- four
    /// `f64` -- comes back in `v0`..`v3` like any other homogeneous float
    /// aggregate, so this one entry point is correct for every send this makes.
    fn objc_msgSend();

    fn CGColorSpaceCreateDeviceRGB() -> Id;
    fn CGColorSpaceRelease(cs: Id);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: *const c_void,
    ) -> Id;
    fn CGDataProviderRelease(p: Id);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: Id,
        bitmap_info: u32,
        provider: Id,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> Id;
    fn CGImageRelease(img: Id);
}

fn msg_ptr() -> *const () {
    objc_msgSend as unsafe extern "C" fn() as *const ()
}

unsafe fn send<R>(obj: Id, sel: Sel) -> R {
    let f: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(msg_ptr());
    f(obj, sel)
}
unsafe fn send1<A, R>(obj: Id, sel: Sel, a: A) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A) -> R = std::mem::transmute(msg_ptr());
    f(obj, sel, a)
}
unsafe fn send2<A, B, R>(obj: Id, sel: Sel, a: A, b: B) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A, B) -> R = std::mem::transmute(msg_ptr());
    f(obj, sel, a, b)
}
unsafe fn send4<A, B, C, D, R>(obj: Id, sel: Sel, a: A, b: B, c: C, d: D) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A, B, C, D) -> R = std::mem::transmute(msg_ptr());
    f(obj, sel, a, b, c, d)
}

fn class(name: &str) -> Id {
    let c = CString::new(name).expect("a class name with a NUL in it");
    unsafe { objc_getClass(c.as_ptr()) }
}

fn sel(name: &str) -> Sel {
    let c = CString::new(name).expect("a selector with a NUL in it");
    unsafe { sel_registerName(c.as_ptr()) }
}

/// An `NSString`, autoreleased. Only used for titles and run-loop modes, both
/// of which are consumed immediately.
fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_else(|_| CString::new("").unwrap());
    unsafe {
        send1::<*const c_char, Id>(
            class("NSString"),
            sel("stringWithUTF8String:"),
            c.as_ptr(),
        )
    }
}

// -------------------------------------------------------------- geometry ---

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CgPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CgSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

// ------------------------------------------------------------- constants ---

const STYLE_TITLED: u64 = 1;
const STYLE_CLOSABLE: u64 = 1 << 1;
const STYLE_MINIATURIZABLE: u64 = 1 << 2;
const STYLE_RESIZABLE: u64 = 1 << 3;
const BACKING_BUFFERED: u64 = 2;
const ACTIVATION_REGULAR: i64 = 0;
/// `NSImageScaleAxesIndependently`. The canvas is already the view's size, so
/// this only matters during the frame between a resize and the next paint.
const SCALE_AXES_INDEPENDENTLY: u64 = 3;

/// `kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little`.
///
/// THE ONE CONSTANT THAT MUST BE EXACTLY RIGHT. The canvas word is
/// `0x00RRGGBB` in native order, which on a little-endian machine is
/// `B G R 00` in memory. These flags read a 32-bit word as `ARGB` and discard
/// the `A`, which is that word -- so there is no conversion pass and no second
/// buffer. Get them wrong and the picture is blue.
const BITMAP_INFO: u32 = 6 | (2 << 12);

const EV_LEFT_DOWN: u64 = 1;
const EV_LEFT_UP: u64 = 2;
const EV_RIGHT_DOWN: u64 = 3;
const EV_RIGHT_UP: u64 = 4;
const EV_MOUSE_MOVED: u64 = 5;
const EV_LEFT_DRAGGED: u64 = 6;
const EV_RIGHT_DRAGGED: u64 = 7;
const EV_KEY_DOWN: u64 = 10;
const EV_KEY_UP: u64 = 11;
const EV_FLAGS_CHANGED: u64 = 12;
const EV_SCROLL: u64 = 22;

const MOD_SHIFT: u64 = 1 << 17;
const MOD_CTRL: u64 = 1 << 18;
const MOD_ALT: u64 = 1 << 19;
const MOD_CMD: u64 = 1 << 20;

// --------------------------------------------------------------- keyboard ---

/// macOS virtual keycode to (`Sym`, PC/AT set-1 scancode).
///
/// The second half of each pair is for Control, which sends set-1 scancodes
/// over RDP and lets the far end do its own keymapping -- see
/// `surface.rs`'s note on why a keypress is two facts.
///
/// Only the named keys are here. Everything else becomes `Sym::Char` from the
/// event's own `characters`, which on a Mac is already composed and carries
/// the whole input-method stack for free. That is the one place this platform
/// is easier than Wayland, where `docs/surface.md` §5's baked table is the
/// entire text layer.
const NAMED: &[(u16, Sym, u16)] = &[
    (0x24, Sym::Return, 0x1c),
    (0x30, Sym::Tab, 0x0f),
    (0x31, Sym::Space, 0x39),
    (0x33, Sym::Backspace, 0x0e),
    (0x35, Sym::Escape, 0x01),
    (0x75, Sym::Delete, 0x53),
    (0x73, Sym::Home, 0x47),
    (0x77, Sym::End, 0x4f),
    (0x74, Sym::PageUp, 0x49),
    (0x79, Sym::PageDown, 0x51),
    (0x7b, Sym::Left, 0x4b),
    (0x7c, Sym::Right, 0x4d),
    (0x7d, Sym::Down, 0x50),
    (0x7e, Sym::Up, 0x48),
    (0x7a, Sym::Func(1), 0x3b),
    (0x78, Sym::Func(2), 0x3c),
    (0x63, Sym::Func(3), 0x3d),
    (0x76, Sym::Func(4), 0x3e),
    (0x60, Sym::Func(5), 0x3f),
    (0x61, Sym::Func(6), 0x40),
    (0x62, Sym::Func(7), 0x41),
    (0x64, Sym::Func(8), 0x42),
    (0x65, Sym::Func(9), 0x43),
    (0x6d, Sym::Func(10), 0x44),
    (0x67, Sym::Func(11), 0x57),
    (0x6f, Sym::Func(12), 0x58),
];

/// The letters, digits and punctuation, as set-1 scancodes on a US layout.
///
/// Used only by Control. The interface itself binds `Sym::Char`, which comes
/// from the event's composed text and is therefore right on every layout.
fn set1_of_char(c: char) -> u16 {
    const ROW: &[(char, u16)] = &[
        ('1', 0x02), ('2', 0x03), ('3', 0x04), ('4', 0x05), ('5', 0x06),
        ('6', 0x07), ('7', 0x08), ('8', 0x09), ('9', 0x0a), ('0', 0x0b),
        ('-', 0x0c), ('=', 0x0d),
        ('q', 0x10), ('w', 0x11), ('e', 0x12), ('r', 0x13), ('t', 0x14),
        ('y', 0x15), ('u', 0x16), ('i', 0x17), ('o', 0x18), ('p', 0x19),
        ('[', 0x1a), (']', 0x1b), ('\\', 0x2b),
        ('a', 0x1e), ('s', 0x1f), ('d', 0x20), ('f', 0x21), ('g', 0x22),
        ('h', 0x23), ('j', 0x24), ('k', 0x25), ('l', 0x26), (';', 0x27),
        ('\'', 0x28), ('`', 0x29),
        ('z', 0x2c), ('x', 0x2d), ('c', 0x2e), ('v', 0x2f), ('b', 0x30),
        ('n', 0x31), ('m', 0x32), (',', 0x33), ('.', 0x34), ('/', 0x35),
    ];
    let lower = c.to_ascii_lowercase();
    ROW.iter().find(|(k, _)| *k == lower).map(|(_, s)| *s).unwrap_or(0)
}

fn mods_of(flags: u64) -> Mods {
    Mods {
        shift: flags & MOD_SHIFT != 0,
        ctrl: flags & MOD_CTRL != 0,
        alt: flags & MOD_ALT != 0,
        logo: flags & MOD_CMD != 0,
    }
}

/// Is this character worth sending as text?
///
/// AppKit puts the arrow keys and the function keys in the Unicode private use
/// area (`U+F700`..) inside `characters`, so a Terminal fed that string would
/// receive four bytes of nonsense for every press of Up. They travel as `Sym`
/// instead, which is where the interface reads them from anyway.
fn is_text(c: char) -> bool {
    // The control range, DEL, and AppKit's private-use block. Written as one
    // `matches!` because the obvious spelling of the first clause --
    // `!(c as u32) < 0x20` -- parses as `(!(c as u32)) < 0x20`, which is
    // bitwise NOT on a u32 and is false for every character there is. The
    // test below is what found it.
    !matches!(c as u32, 0x00..=0x1f | 0x7f | 0xf700..=0xf8ff)
}

// ----------------------------------------------------------------- window ---

pub struct Window {
    app: Id,
    window: Id,
    view: Id,
    colour_space: Id,
    /// The canvas, in DEVICE pixels -- `points * scale`.
    px: Vec<u8>,
    w: usize,
    h: usize,
    scale: f64,
    /// Set when the content view's frame no longer matches the canvas.
    pending: Option<(usize, usize)>,
    closed: bool,
    /// The image the view is holding. Released when the next one replaces it.
    last_image: Id,
    default_mode: Id,
    distant_past: Id,
}

impl Window {
    pub fn open(title: &str, w: usize, h: usize) -> Result<Window, String> {
        unsafe {
            let app: Id = send(class("NSApplication"), sel("sharedApplication"));
            if app.is_null() {
                return Err("NSApplication would not start -- is there a window server?".into());
            }
            // Regular, so the window can take focus and appear in the Dock.
            // Without it a plain binary is an "accessory" and opens a window
            // nothing can type into.
            let _: bool = send1(app, sel("setActivationPolicy:"), ACTIVATION_REGULAR);
            let _: () = send(app, sel("finishLaunching"));

            let frame = CgRect {
                origin: CgPoint { x: 0.0, y: 0.0 },
                size: CgSize { width: w as f64, height: h as f64 },
            };
            let style = STYLE_TITLED | STYLE_CLOSABLE | STYLE_MINIATURIZABLE | STYLE_RESIZABLE;

            let window: Id = send(class("NSWindow"), sel("alloc"));
            let window: Id = send4(
                window,
                sel("initWithContentRect:styleMask:backing:defer:"),
                frame,
                style,
                BACKING_BUFFERED,
                false,
            );
            if window.is_null() {
                return Err("NSWindow would not open".into());
            }
            let _: () = send1(window, sel("setTitle:"), nsstring(title));
            // Without this there are no NSEventTypeMouseMoved events at all and
            // nothing on the interface is ever hot.
            let _: () = send1(window, sel("setAcceptsMouseMovedEvents:"), true);
            let _: () = send1(window, sel("setReleasedWhenClosed:"), false);

            let view: Id = send(class("NSImageView"), sel("alloc"));
            let view: Id = send1(view, sel("initWithFrame:"), frame);
            let _: () = send1(view, sel("setImageScaling:"), SCALE_AXES_INDEPENDENTLY);
            let _: () = send1(window, sel("setContentView:"), view);

            let _: () = send(window, sel("center"));
            let _: () = send1(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut::<c_void>());
            // A binary that is not inside a .app bundle opens behind the
            // terminal it was started from without this.
            let _: () = send1(app, sel("activateIgnoringOtherApps:"), true);

            let scale: f64 = send(window, sel("backingScaleFactor"));
            let scale = if scale >= 1.0 { scale } else { 1.0 };
            let (dw, dh) = ((w as f64 * scale) as usize, (h as f64 * scale) as usize);

            let distant_past: Id = send(class("NSDate"), sel("distantPast"));

            Ok(Window {
                app,
                window,
                view,
                colour_space: CGColorSpaceCreateDeviceRGB(),
                px: vec![0; dw * dh * 4],
                w: dw,
                h: dh,
                scale,
                pending: None,
                closed: false,
                last_image: std::ptr::null_mut(),
                default_mode: nsstring("kCFRunLoopDefaultMode"),
                distant_past,
            })
        }
    }

    /// The backing scale. `1.0` on an old panel, `2.0` on every Mac worth using.
    ///
    /// The canvas is allocated at `points * scale` and the `NSImage` is given
    /// a size in POINTS -- the one place in the program where those two numbers
    /// differ, and the difference between a console and a photograph of one.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    fn content_size(&self) -> (usize, usize) {
        unsafe {
            let r: CgRect = send(self.view, sel("frame"));
            (
                (r.size.width * self.scale) as usize,
                (r.size.height * self.scale) as usize,
            )
        }
    }

    /// Turn one NSEvent into whatever the interface should hear, and hand it
    /// on to AppKit so that dragging the title bar still works.
    unsafe fn translate(&self, e: Id, out: &mut Vec<Input>) {
        let kind: u64 = send(e, sel("type"));
        let flags: u64 = send(e, sel("modifierFlags"));
        let mods = mods_of(flags);

        // The pointer, in canvas coordinates. AppKit's origin is the bottom
        // left of the content view and the canvas's is the top left, so the
        // y axis is flipped here and nowhere else.
        let at = || -> (i32, i32) {
            let p: CgPoint = send(e, sel("locationInWindow"));
            let r: CgRect = send(self.view, sel("frame"));
            (
                (p.x * self.scale) as i32,
                ((r.size.height - p.y) * self.scale) as i32,
            )
        };

        match kind {
            EV_MOUSE_MOVED | EV_LEFT_DRAGGED | EV_RIGHT_DRAGGED => {
                let (x, y) = at();
                out.push(Input::Motion { x, y });
            }
            EV_LEFT_DOWN | EV_LEFT_UP | EV_RIGHT_DOWN | EV_RIGHT_UP => {
                let (x, y) = at();
                let button = if kind == EV_LEFT_DOWN || kind == EV_LEFT_UP {
                    Button::Left
                } else {
                    Button::Right
                };
                let down = kind == EV_LEFT_DOWN || kind == EV_RIGHT_DOWN;
                // The modifiers ride with the click, so `ui.rs` does not have
                // to guess which key event established them.
                out.push(Input::Key {
                    scancode: 0,
                    sym: Sym::Unknown,
                    down: true,
                    mods,
                });
                out.push(Input::Button { x, y, button, down });
            }
            EV_SCROLL => {
                let dx: f64 = send(e, sel("scrollingDeltaX"));
                let dy: f64 = send(e, sel("scrollingDeltaY"));
                out.push(Input::Scroll { dx: dx as i32, dy: dy as i32 });
            }
            EV_KEY_DOWN | EV_KEY_UP => {
                let down = kind == EV_KEY_DOWN;
                let code: u16 = send(e, sel("keyCode"));
                let chars: Id = send(e, sel("characters"));
                let text = if chars.is_null() {
                    String::new()
                } else {
                    let p: *const c_char = send(chars, sel("UTF8String"));
                    if p.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(p).to_string_lossy().into_owned()
                    }
                };

                let (sym, scancode) = match NAMED.iter().find(|(k, _, _)| *k == code) {
                    Some((_, s, sc)) => (*s, *sc),
                    None => match text.chars().next().filter(|c| is_text(*c)) {
                        Some(c) => (Sym::Char(c.to_ascii_lowercase()), set1_of_char(c)),
                        None => (Sym::Unknown, 0),
                    },
                };
                out.push(Input::Key { scancode, sym, down, mods });

                // Text only on the way down, and never for a chord: Command-A
                // is "select all", not the letter a.
                if down && !mods.ctrl && !mods.logo {
                    let keep: String = text.chars().filter(|c| is_text(*c)).collect();
                    if !keep.is_empty() {
                        out.push(Input::Text(keep));
                    }
                }
            }
            EV_FLAGS_CHANGED => {
                // Shift on its own is not a keypress, but Control needs to know
                // the modifier went down -- and the seat's own lesson is that
                // a modifier which is never released stays held on the far end.
                out.push(Input::Key {
                    scancode: 0,
                    sym: Sym::Unknown,
                    down: true,
                    mods,
                });
            }
            _ => {}
        }

        let _: () = send1(self.app, sel("sendEvent:"), e);
    }
}

impl Surface for Window {
    fn pixels(&mut self) -> &mut [u8] {
        &mut self.px
    }

    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    fn poll(&mut self) -> Result<Vec<Input>, String> {
        let mut out = Vec::new();
        unsafe {
            // Drain every event that is already queued. `distantPast` makes
            // this return nil the moment there are none, so it never blocks.
            loop {
                let e: Id = send4(
                    self.app,
                    sel("nextEventMatchingMask:untilDate:inMode:dequeue:"),
                    u64::MAX,
                    self.distant_past,
                    self.default_mode,
                    true,
                );
                if e.is_null() {
                    break;
                }
                self.translate(e, &mut out);
            }

            // The two that are polled rather than delivered.
            let visible: bool = send(self.window, sel("isVisible"));
            if !visible && !self.closed {
                self.closed = true;
                out.push(Input::Closed);
            }
        }

        let (cw, ch) = self.content_size();
        if cw > 0 && ch > 0 && (cw != self.w || ch != self.h) {
            self.pending = Some((cw, ch));
            out.push(Input::Resized { w: cw, h: ch });
        }
        Ok(out)
    }

    fn apply_resize(&mut self) -> Result<(), String> {
        if let Some((w, h)) = self.pending.take() {
            self.w = w;
            self.h = h;
            self.px.resize(w * h * 4, 0);
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), String> {
        unsafe {
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                self.px.as_ptr() as *const c_void,
                self.w * self.h * 4,
                // No release callback: the buffer is this struct's and outlives
                // every image made from it.
                std::ptr::null(),
            );
            if provider.is_null() {
                return Err("CGDataProviderCreateWithData failed".into());
            }
            let image = CGImageCreate(
                self.w,
                self.h,
                8,
                32,
                self.w * 4,
                self.colour_space,
                BITMAP_INFO,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            CGDataProviderRelease(provider);
            if image.is_null() {
                return Err("CGImageCreate failed".into());
            }

            // THE SIZE IS IN POINTS AND THE IMAGE IS IN DEVICE PIXELS. This is
            // the whole of Retina support, and getting it backwards is a
            // console at half size in the corner of its own window.
            let size = CgSize {
                width: self.w as f64 / self.scale,
                height: self.h as f64 / self.scale,
            };
            let ns: Id = send(class("NSImage"), sel("alloc"));
            let ns: Id = send2(ns, sel("initWithCGImage:size:"), image, size);

            let _: () = send1(self.view, sel("setImage:"), ns);
            let _: () = send1(self.view, sel("setNeedsDisplay:"), true);
            let _: () = send(ns, sel("release"));

            if !self.last_image.is_null() {
                CGImageRelease(self.last_image);
            }
            self.last_image = image;
        }
        Ok(())
    }

    fn closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            if !self.last_image.is_null() {
                CGImageRelease(self.last_image);
            }
            if !self.colour_space.is_null() {
                CGColorSpaceRelease(self.colour_space);
            }
            let _: () = send(self.window, sel("close"));
            let _: () = send(self.window, sel("release"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run with no window server -- they are about the tables and the
    // arithmetic, which is where the mistakes that survive a first look live.

    #[test]
    fn the_bitmap_flags_are_the_ones_that_match_the_canvas_word() {
        // kCGImageAlphaNoneSkipFirst is 6, kCGBitmapByteOrder32Little is 2<<12.
        // A canvas word of 0x00RRGGBB is B,G,R,00 in memory, which these flags
        // read as ARGB with the A discarded. Any other pair means a blue
        // console, so the value is asserted rather than trusted.
        assert_eq!(BITMAP_INFO, 8198);
        assert_eq!(BITMAP_INFO & 0x1f, 6, "the alpha info is not SkipFirst");
        assert_eq!(BITMAP_INFO >> 12, 2, "the byte order is not 32Little");
    }

    #[test]
    fn every_modifier_is_the_bit_appkit_actually_sets() {
        assert_eq!(mods_of(MOD_SHIFT), Mods { shift: true, ..Default::default() });
        assert_eq!(mods_of(MOD_CTRL), Mods { ctrl: true, ..Default::default() });
        assert_eq!(mods_of(MOD_ALT), Mods { alt: true, ..Default::default() });
        assert_eq!(mods_of(MOD_CMD), Mods { logo: true, ..Default::default() });
        // Command is the toggling chord on this platform, Control on the other.
        assert!(mods_of(MOD_CMD).toggling());
        // AppKit sets device-dependent bits in the low half; they must not
        // become modifiers.
        assert_eq!(mods_of(0x102), Mods::default());
    }

    #[test]
    fn the_named_keys_are_unique_in_both_directions() {
        for (i, (code, sym, sc)) in NAMED.iter().enumerate() {
            for (other, osym, osc) in NAMED.iter().skip(i + 1) {
                assert_ne!(code, other, "two entries for virtual key {:#x}", code);
                assert_ne!(sym, osym, "two virtual keys map to {:?}", sym);
                assert_ne!(sc, osc, "two keys share set-1 scancode {:#x}", sc);
            }
        }
    }

    #[test]
    fn the_arrow_keys_carry_the_scancodes_rdp_expects() {
        let find = |s: Sym| NAMED.iter().find(|(_, k, _)| *k == s).map(|(_, _, sc)| *sc);
        // The grey arrow block, set 1. Control forwards these verbatim.
        assert_eq!(find(Sym::Up), Some(0x48));
        assert_eq!(find(Sym::Left), Some(0x4b));
        assert_eq!(find(Sym::Right), Some(0x4d));
        assert_eq!(find(Sym::Down), Some(0x50));
        assert_eq!(find(Sym::Escape), Some(0x01));
        assert_eq!(find(Sym::Return), Some(0x1c));
    }

    #[test]
    fn the_letters_map_to_their_set_one_scancodes_on_either_case() {
        assert_eq!(set1_of_char('a'), 0x1e);
        assert_eq!(set1_of_char('A'), 0x1e, "shift changed the physical key");
        assert_eq!(set1_of_char('z'), 0x2c);
        assert_eq!(set1_of_char('1'), 0x02);
        assert_eq!(set1_of_char('0'), 0x0b);
        // Unknown is zero rather than a wrong key: Control sending the wrong
        // scancode is worse than Control sending none.
        assert_eq!(set1_of_char('\u{20ac}'), 0);
    }

    #[test]
    fn appkits_private_use_characters_never_become_text() {
        // NSEvent.characters puts the arrows at U+F700.., so a Terminal fed
        // that string would get nonsense for every press of Up.
        for c in ['\u{f700}', '\u{f701}', '\u{f8ff}', '\u{1b}', '\u{7f}', '\u{0}'] {
            assert!(!is_text(c), "{:?} would have been sent as text", c);
        }
        for c in ['a', 'Z', '0', ' ', '/', '\u{e9}'] {
            assert!(is_text(c), "{:?} should be text", c);
        }
    }

    #[test]
    fn a_retina_canvas_is_points_times_scale_and_the_image_is_points() {
        // The arithmetic present() does, checked without a window server.
        let (points_w, points_h, scale) = (960.0f64, 600.0f64, 2.0f64);
        let (dw, dh) = ((points_w * scale) as usize, (points_h * scale) as usize);
        assert_eq!((dw, dh), (1920, 1200));
        assert_eq!(dw as f64 / scale, points_w);
        assert_eq!(dh as f64 / scale, points_h);
        assert_eq!(dw * dh * 4, 9_216_000, "the canvas is not four bytes a pixel");
    }
}
