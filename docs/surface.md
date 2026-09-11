# One canvas, two platforms

*How a window gets opened on a Raspberry Pi and on a Mac without the drawing
knowing which.*

`draw.rs` already has no idea what a window is. It takes `&mut [u8]`, a width
and a height, and writes pixels. That was luck as much as design — phase 2 wrote
a software canvas because a Zero 2 should not carry an EGL stack — but it is
what makes a second platform a new file rather than a new program.

What is platform-specific is four things: getting a rectangle of memory the
compositor will show, being told it changed size, being told the user closed it,
and being told what the user did. Only the fourth is missing today, and it is
missing on both.

---

## 1 · The seam

```rust
/// A rectangle of pixels somebody else is willing to put on a screen.
pub trait Surface {
    fn open(title: &str, w: usize, h: usize) -> Result<Self, Error> where Self: Sized;

    /// `width * height` words of 0x00RRGGBB, native-endian. The canvas.
    fn pixels(&mut self) -> &mut [u8];
    fn size(&self) -> (usize, usize);

    /// Everything that has happened since the last call. Never blocks.
    fn poll(&mut self) -> Result<Vec<Input>, Error>;

    /// Adopt a size the platform asked for, reallocating the buffer.
    fn apply_resize(&mut self) -> Result<(), Error>;

    /// Hand the pixels over. May be a no-op while the platform holds them.
    fn present(&mut self) -> Result<(), Error>;

    fn closed(&self) -> bool;
    fn busy(&self) -> bool;
}
```

`wl::Window` already has `pixels`, `apply_resize`, `present`, `closed` and
`buffer_busy` with these exact meanings, so the Wayland side of this is a
rename and an `impl` block. `poll` changes shape — it returns events instead of
a `Changes` struct with two booleans — and the two booleans become
`Input::Resized` and `Input::Closed`.

**The trait is not there for a third platform.** There will not be one. It is
there so that the museum interface can be written once against a thing that
produces `Input` and accepts pixels, and so that `--frame` — which has no
platform at all — remains a first-class way to look at the drawing.

---

## 2 · Input, as the interface sees it

```rust
pub enum Input {
    Motion  { x: i32, y: i32 },
    Button  { x: i32, y: i32, button: Button, down: bool },
    Scroll  { dx: i32, dy: i32 },
    /// A physical key. `scancode` is PC/AT set 1, which is what RDP wants.
    Key     { scancode: u16, sym: Sym, down: bool, mods: Mods },
    /// What that key means as text, when it means any. UTF-8, already composed.
    Text    (String),
    Resized { w: usize, h: usize },
    Closed,
    /// The platform thinks now is a good time to draw.
    Frame,
}
```

**`Key` and `Text` are separate and both are needed**, which is the one part of
this that is not obvious. Terminal wants characters — a shell is fed UTF-8.
Control wants *scancodes* — RDP's fastpath input events carry PC/AT set-1 scan
codes and the far end does its own keymapping. The same keypress is therefore
two facts, and an interface that kept only one of them would have to
reconstruct the other badly. So both travel.

`Mods` is the four that matter (shift, ctrl, alt, logo) as a bitfield.
`Sym` is an enum of the named keys — arrows, function keys, Escape, Return, Tab,
Backspace, Delete, Home/End/PageUp/PageDown — plus `Sym::Char(char)` for
everything else. This is deliberately not X11 keysym numbering: nothing here
needs to interoperate with an X server, and a small enum is a small enum.

---

## 3 · The Wayland side — what `wl.rs` gains

Today `wl.rs` binds three globals: `wl_compositor`, `wl_shm`, `xdg_wm_base`.
Input needs a fourth and its three children.

| interface | version | what for |
|---|---|---|
| `wl_seat` | 5 | the capability event, which says whether there is a pointer and a keyboard at all |
| `wl_pointer` | 5 | enter/leave/motion/button/axis |
| `wl_keyboard` | 5 | keymap, enter/leave, key, modifiers, repeat_info |

Roughly 300 lines in the existing idiom: opcodes as `const`, events decoded
through `Args`. Two pieces are worth naming because they are where this goes
wrong:

**Coordinates are fixed-point.** `wl_pointer.motion` carries `wl_fixed` — a
24.8 signed fixed-point number, not an integer. `Args` has `int()` and `uint()`
and needs a `fixed()` that shifts right by 8. Reading it as an `i32` puts the
pointer at 256 times the right place, which is the kind of bug that looks like
a broken compositor.

**The keymap arrives as a file descriptor, and it is not going to be read.**
`wl_keyboard.keymap` sends `XKB_V1` — a text keymap, in the file descriptor —
and the correct thing to do with it is hand it to `libxkbcommon`. That is a
dependency, on the one machine that cannot fetch one, so this does what
`font.rs` did with the font: **bakes a table in.** See §5.

The compositor also sends `wl_keyboard.repeat_info` with a rate and a delay,
and honouring it is the difference between a held arrow key scrolling and a
held arrow key doing nothing. It is a timer in the event loop, not a protocol
feature.

---

## 4 · The Mac side — `src/mac.rs`

A Mac cannot run `--gui` today and that is the single reason the operator's own
machine — the one with `answers.txt` on it, the one that writes SD cards, the
one `copal-prep.sh` runs on — has never been able to open this console.

### What it is, and what it refuses to be

AppKit, reached through the Objective-C runtime by declaring its functions with
`unsafe extern "C"`, exactly as `sys.rs` declares nine functions from libc.
Nothing is fetched and nothing is vendored; `AppKit`, `Foundation`,
`CoreGraphics` and `libobjc` are on every Mac and are linked as frameworks.

**No Objective-C classes are created at runtime, and that is the design.** The
usual way to do this — `objc_allocateClassPair`, `class_addMethod`, a
`drawRect:` implementation, a window delegate — is a large amount of very
fiddly unsafe code, and it exists to let AppKit call back *into* Rust. This
program does not need callbacks. It has a loop:

| need | how, without a subclass |
|---|---|
| a window | `NSWindow initWithContentRect:styleMask:backing:defer:` |
| pixels on it | `NSImageView` as the content view; set its `image` each frame |
| the pixels themselves | `CGDataProviderCreateWithData` → `CGImageCreate` → `NSImage initWithCGImage:size:` |
| events | `[NSApp nextEventMatchingMask:untilDate:inMode:dequeue:]`, polled with a distant-past date so it never blocks |
| "the user closed it" | `[window isVisible]` went false |
| a resize | `[window contentView].frame` changed since last frame |

Every one of those is a message send with a known signature. The resize and the
close are *polled* rather than delivered, which is slightly crude and entirely
sufficient at the frame rates this draws at.

### The three things that are easy to get wrong

**`objc_msgSend` must be transmuted to the exact signature of each call.** It is
declared variadic in the headers but is not a variadic function; calling it
through one prototype for every call site produces argument corruption that
depends on the argument types. Each call site gets its own
`type Fn = unsafe extern "C" fn(Id, Sel, …) -> …` and a transmute. On arm64
there is no `objc_msgSend_stret` and no `_fpret` to worry about, and an
`NSRect` — four `f64` — comes back in `v0`–`v3` like any other homogeneous
float aggregate, so the plain entry point is correct for every call this makes.

**The pixel format lines up exactly, and only with the right flags.** The canvas
word is `0x00RRGGBB` in native order, which on a little-endian machine is
`B G R 00` in memory. `CGImageCreate` with
`kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little` reads a 32-bit word as
`ARGB` and discards the `A`, which is that word. No conversion pass, no second
buffer. Get the flags wrong and the picture is blue.

**A Retina screen is not the size it says it is.** `[window backingScaleFactor]`
is 2 on every Mac worth using, and a canvas allocated at the point size is
scaled up by AppKit and looks like a photograph of a console. The canvas is
allocated at `points × scale` and the `NSImage` is given a size in *points*,
which is the one place in the program where those two numbers differ. The
8×13 font then renders at 8×13 device pixels on a Retina panel, which is small;
`--scale 2` draws every primitive at double size, and is worth having anyway
for the gallery screen at the far end of a room.

**AppKit wants the main thread.** `main()` calls into `mac.rs` directly and the
loop runs there. `NSApplication sharedApplication`,
`setActivationPolicy:NSApplicationActivationPolicyRegular`, `finishLaunching`,
and — because this binary is not in a `.app` bundle —
`[NSApp activateIgnoringOtherApps:YES]`, or the window opens behind the
terminal it was started from.

---

## 5 · The keyboard, baked

`font.rs` carries two X11 fonts because a node has no font package. The same
argument applies one layer over: a node has no `libxkbcommon`, and a Mac has no
evdev.

`src/keymap.rs` carries three tables, generated by `tools/mkkeymap.py` from
`/usr/share/X11/xkb` and Apple's `HIToolbox` virtual key list, and committed the
way `font.rs` is committed:

| table | from | to | used by |
|---|---|---|---|
| `EVDEV_TO_SET1` | Linux evdev keycode − 8 | PC/AT set-1 scancode | Wayland → RDP |
| `MACVK_TO_SET1` | macOS virtual keycode | PC/AT set-1 scancode | Mac → RDP |
| `US_LEVELS` | set-1 scancode | unshifted / shifted character | Wayland → text |

macOS does not need the third: `NSEvent.characters` is already the composed
string, including dead keys and the entire input-method stack, for free.
Wayland gives nothing but a keycode, so on Linux the third table is the whole
of the text layer.

**The consequence, stated plainly:** on Linux, a non-US layout types the wrong
letters, and a compose key does nothing. `--keymap us` is the only value that
works and is the default. This is a real limitation and the alternative is
either `libxkbcommon` — a dependency — or an xkb parser, which is a larger and
much less interesting program than the one this is. Recorded here rather than
discovered later.

---

## 6 · `src/ui.rs` — the widgets

Immediate mode. There is no widget tree, no retained state, no callbacks, no
allocation per frame. A frame is:

```rust
let mut ui = Ui::begin(&mut canvas, &theme, &inputs, &mut state);
if ui.button(rect(16, 8, 90, 24), "Control", enabled) { control(&selection); }
ui.end();
```

`Ui` holds the canvas, the theme, this frame's inputs, and a small persistent
`UiState` — the hot and active widget ids, the focused widget, the scroll
offsets, and the text cursor. Widget identity is the rectangle plus a caller-
supplied salt, which is enough when the layout is computed rather than dynamic.

| widget | for |
|---|---|
| `button` | the verb bar, the pane buttons |
| `toggle` | `--dark`, `--scale`, the Observe/Control switch |
| `tile` | a node. The one widget that is about this program rather than about widgets. |
| `list` / `rail_item` | scenes, tags, the card ledger, a directory in Exchange |
| `field` | the Send path, the Run argument, the Message text |
| `pane` | a region with a title bar and a close box, filling the lab |
| `term` | a character grid — the Terminal's and the results pane's shared drawing |
| `screen` | a framebuffer scaled into a rectangle — Observe and Control |

**Why immediate mode, on this program specifically.** A retained tree costs
allocations and a diff, and buys the ability to update one widget without
redrawing. This console redraws entirely, at 6 frames a second, into a buffer it
already owns — `draw.rs` fills a 960×600 canvas in well under a millisecond on
a Zero 2, because it is rectangles and 8×13 glyphs. There is nothing for a
retained tree to save, and there is a whole category of bug — stale state in a
node that no longer corresponds to a machine in the fleet — that immediate mode
cannot have.

`term` and `screen` are the two that are not cheap, and they are the two that
already exist in another form: `paint.rs` scales a framebuffer into a terminal,
and `seat.rs` draws a character grid. The scaling arithmetic is shared with
`paint::fit`.

---

## 7 · The loop

```
open a surface
loop {
    inputs = surface.poll()
    if closed, stop
    if resized, apply_resize
    state = read model, if it is older than the refresh interval
    canvas = Canvas::new(surface.pixels(), w, h)
    draw the interface, consuming inputs
    surface.present()
    wait for the next frame, or for input
}
```

The read model is refreshed on a timer — five seconds, the web page's interval —
in a **second thread**, because `copal fleet state` takes up to forty-five
seconds and a console that stops drawing while it asks is worse than one showing
a five-second-old picture. The thread posts a new document into a `Mutex`; the
loop takes whatever is there. This is the first thread in the program that is
not a connection handler, and it is the reason `Fleet` already serialises its
verbs behind a lock.

Waiting is the one genuinely platform-specific piece left: Wayland has a socket
fd to poll, AppKit has `nextEventMatchingMask:` with a real date. Both become
`Surface::wait_until(deadline)`.

---

## 8 · How it is looked at

`--frame PATH` renders one frame to a PPM and exits, and it keeps doing so.
With an interface rather than a specimen it gains arguments — `--frame-state
FILE` to render against a fixture, `--frame-select a,b` to render with a
selection — so that every screen in this document is a file that can be
regenerated, diffed, and pasted into a review. **A console whose screens can
only be seen by standing in front of one is a console nobody reviews.**

`tools/wl-check.sh` runs the compositor-backed tests under weston headless. It
gains the input tests, which drive `wl_pointer` and `wl_keyboard` through a
second client — the same shape `rfb.rs` used when it grew a `fake` server to
prove the seat against.
