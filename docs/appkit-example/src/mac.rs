//! An AppKit window, by message sends alone.
//!
//! No Objective-C class is defined: AppKit never calls into Rust. The program
//! owns the loop, and everything it needs is a message it sends or a thing
//! it polls:
//!
//!     a window          NSWindow initWithContentRect:styleMask:backing:defer:
//!     pixels on it      an NSImageView, given a new NSImage each frame
//!     the pixels        CGDataProviderCreateWithData -> CGImageCreate
//!     events            nextEventMatchingMask: with a date in the past
//!     closed            [window isVisible] went false
//!     resized           the view's frame no longer matches the canvas
//!
//! Every poll and present runs inside an autorelease pool, because AppKit's
//! own run loop, which would drain one each turn, is not running.
//!
//! arm64: an NSRect comes back in registers through plain objc_msgSend. On
//! Intel a struct that large needs objc_msgSend_stret, which this does not
//! declare.
#![cfg(target_os = "macos")]

use crate::surface::{Input, Surface};
use std::ffi::{c_char, c_void, CStr, CString};

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "AppKit", kind = "framework")]
#[link(name = "Foundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    /// Never called with this prototype: each send below casts it to the
    /// exact signature of that message. (Its header calls it variadic, and
    /// calling it as variadic corrupts arguments.)
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);

    fn CGColorSpaceCreateDeviceRGB() -> Id;
    fn CGDataProviderCreateWithData(info: *mut c_void, data: *const c_void, size: usize, release: *const c_void) -> Id;
    fn CGDataProviderRelease(p: Id);
    #[allow(clippy::too_many_arguments)]
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

// One function per argument count, each a cast of objc_msgSend.
unsafe fn send<R>(o: Id, s: Sel) -> R {
    let f: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, s)
}
unsafe fn send1<A, R>(o: Id, s: Sel, a: A) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A) -> R = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, s, a)
}
unsafe fn send2<A, B, R>(o: Id, s: Sel, a: A, b: B) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A, B) -> R = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, s, a, b)
}
unsafe fn send4<A, B, C, D, R>(o: Id, s: Sel, a: A, b: B, c: C, d: D) -> R {
    let f: unsafe extern "C" fn(Id, Sel, A, B, C, D) -> R = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(o, s, a, b, c, d)
}

fn class(name: &str) -> Id {
    let c = CString::new(name).unwrap();
    unsafe { objc_getClass(c.as_ptr()) }
}
fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}
/// An autoreleased NSString: used at once, inside a pool.
unsafe fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap();
    send1(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr())
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    origin: Point,
    size: Size,
}

const STYLE: u64 = 1 | 2 | 4 | 8; // titled, closable, miniaturizable, resizable
const BACKING_BUFFERED: u64 = 2;
const ACTIVATION_REGULAR: i64 = 0;
/// The canvas word 0x00RRGGBB, little-endian, is B G R 0 in memory: read as
/// "alpha first, skipped" in 32-bit little-endian order it needs no
/// conversion (kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little).
const BITMAP_INFO: u32 = 6 | (2 << 12);
const EV_LEFT_DOWN: u64 = 1;
const EV_MOUSE_MOVED: u64 = 5;
const EV_KEY_DOWN: u64 = 10;
const MOD_CMD: u64 = 1 << 20;
const KEY_ESCAPE: u16 = 53;

pub struct Window {
    app: Id,
    window: Id,
    view: Id,
    space: Id,
    px: Vec<u32>,
    w: usize,
    h: usize,
    scale: f64,
    resized: bool,
    closed: bool,
    last_image: Id,
    mode: Id,
    past: Id,
}

impl Window {
    /// A window `w` x `h` points; its canvas is in device pixels.
    pub fn open(title: &str, w: usize, h: usize) -> Window {
        unsafe {
            let app: Id = send(class("NSApplication"), sel("sharedApplication"));
            // Regular: a Dock icon and keyboard focus, even run from a terminal.
            let _: bool = send1(app, sel("setActivationPolicy:"), ACTIVATION_REGULAR);
            let _: () = send(app, sel("finishLaunching"));

            let frame = Rect { origin: Point::default(), size: Size { w: w as f64, h: h as f64 } };
            let window: Id = send(class("NSWindow"), sel("alloc"));
            let window: Id = send4(window, sel("initWithContentRect:styleMask:backing:defer:"), frame, STYLE, BACKING_BUFFERED, false);
            let pool = objc_autoreleasePoolPush();
            let _: () = send1(window, sel("setTitle:"), nsstring(title));
            let mode = nsstring("kCFRunLoopDefaultMode");
            let _: Id = send(mode, sel("retain"));
            objc_autoreleasePoolPop(pool);
            let _: () = send1(window, sel("setAcceptsMouseMovedEvents:"), true);
            let _: () = send1(window, sel("setReleasedWhenClosed:"), false);

            let view: Id = send(class("NSImageView"), sel("alloc"));
            let view: Id = send1(view, sel("initWithFrame:"), frame);
            let _: () = send1(view, sel("setImageScaling:"), 3u64); // axes independently
            let _: () = send1(window, sel("setContentView:"), view);
            let _: () = send(window, sel("center"));
            let _: () = send1(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut::<c_void>());
            let _: () = send1(app, sel("activateIgnoringOtherApps:"), true);

            let scale: f64 = send(window, sel("backingScaleFactor"));
            let scale = scale.max(1.0);
            let (dw, dh) = ((w as f64 * scale) as usize, (h as f64 * scale) as usize);
            Window {
                app,
                window,
                view,
                space: CGColorSpaceCreateDeviceRGB(),
                px: vec![0; dw * dh],
                w: dw,
                h: dh,
                scale,
                resized: false,
                closed: false,
                last_image: std::ptr::null_mut(),
                mode,
                past: send(class("NSDate"), sel("distantPast")),
            }
        }
    }

    /// The view's size in device pixels.
    unsafe fn view_size(&self) -> (usize, usize) {
        let r: Rect = send(self.view, sel("frame"));
        ((r.size.w * self.scale) as usize, (r.size.h * self.scale) as usize)
    }

    /// One NSEvent, in the program's words. AppKit's origin is bottom left;
    /// the canvas's is top left, so y is flipped here and only here.
    unsafe fn translate(&self, e: Id, out: &mut Vec<Input>) {
        let kind: u64 = send(e, sel("type"));
        let at = || {
            let p: Point = send(e, sel("locationInWindow"));
            let r: Rect = send(self.view, sel("frame"));
            ((p.x * self.scale) as i32, ((r.size.h - p.y) * self.scale) as i32)
        };
        match kind {
            EV_MOUSE_MOVED => {
                let (x, y) = at();
                out.push(Input::Moved { x, y });
            }
            EV_LEFT_DOWN => {
                let (x, y) = at();
                out.push(Input::Clicked { x, y });
            }
            EV_KEY_DOWN => {
                let code: u16 = send(e, sel("keyCode"));
                let flags: u64 = send(e, sel("modifierFlags"));
                if code == KEY_ESCAPE {
                    out.push(Input::Escape);
                } else {
                    let s: Id = send(e, sel("charactersIgnoringModifiers"));
                    let p: *const c_char = send(s, sel("UTF8String"));
                    if !p.is_null() {
                        if let Some(ch) = CStr::from_ptr(p).to_str().unwrap_or("").chars().next() {
                            out.push(Input::Key { ch, cmd: flags & MOD_CMD != 0 });
                        }
                    }
                }
                // A key the program took is not handed on: with no menu,
                // AppKit would only beep at it.
                return;
            }
            _ => {}
        }
        // Everything else goes on to AppKit, so the title bar still drags and
        // the close button still closes.
        let _: () = send1(self.app, sel("sendEvent:"), e);
    }
}

impl Surface for Window {
    fn pixels(&mut self) -> &mut [u32] {
        &mut self.px
    }
    fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    fn poll(&mut self) -> Vec<Input> {
        let mut out = Vec::new();
        unsafe {
            let pool = objc_autoreleasePoolPush();
            loop {
                let e: Id = send4(
                    self.app,
                    sel("nextEventMatchingMask:untilDate:inMode:dequeue:"),
                    u64::MAX,
                    self.past,
                    self.mode,
                    true,
                );
                if e.is_null() {
                    break;
                }
                self.translate(e, &mut out);
            }
            let _: () = send(self.app, sel("updateWindows"));
            let visible: bool = send(self.window, sel("isVisible"));
            if !visible && !self.closed {
                self.closed = true;
                out.push(Input::Closed);
            }
            let (vw, vh) = self.view_size();
            if vw > 0 && vh > 0 && (vw, vh) != (self.w, self.h) {
                self.resized = true;
                out.push(Input::Resized);
            }
            objc_autoreleasePoolPop(pool);
        }
        out
    }

    fn apply_resize(&mut self) {
        if self.resized {
            let (w, h) = unsafe { self.view_size() };
            self.w = w;
            self.h = h;
            self.px = vec![0; w * h];
            self.resized = false;
        }
    }

    fn present(&mut self) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            // The image reads the canvas in place: no copy, no conversion.
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                self.px.as_ptr() as *const c_void,
                self.w * self.h * 4,
                std::ptr::null(),
            );
            let image = CGImageCreate(self.w, self.h, 8, 32, self.w * 4, self.space, BITMAP_INFO, provider, std::ptr::null(), false, 0);
            CGDataProviderRelease(provider);
            // The size is in points, the image in device pixels: that is the
            // whole of Retina support.
            let size = Size { w: self.w as f64 / self.scale, h: self.h as f64 / self.scale };
            let ns: Id = send(class("NSImage"), sel("alloc"));
            let ns: Id = send2(ns, sel("initWithCGImage:size:"), image, size);
            let _: () = send1(self.view, sel("setImage:"), ns);
            let _: () = send(ns, sel("release"));
            // The view holds the new image; the one before can go.
            if !self.last_image.is_null() {
                CGImageRelease(self.last_image);
            }
            self.last_image = image;
            objc_autoreleasePoolPop(pool);
        }
    }
}
