//! The widgets, immediate mode.
//!
//! There is no widget tree, no retained state, no callbacks, and no allocation
//! per frame. A frame is a pure function of (canvas, theme, inputs, state),
//! which is why every widget here is tested with no compositor and no window:
//! a button press is three lines.
//!
//! WHY IMMEDIATE MODE, ON THIS PROGRAM SPECIFICALLY. A retained tree costs
//! allocations and a diff, and buys the ability to update one widget without
//! redrawing. This console redraws entirely, at six frames a second, into a
//! buffer it already owns -- `draw.rs` fills a 960x600 canvas in well under a
//! millisecond, because it is rectangles and 8x13 glyphs. There is nothing for
//! a retained tree to save, and there is a whole category of bug it would let
//! in: state in a node that no longer corresponds to a machine in the fleet.

use crate::draw::{Canvas, Rgb, Theme};
use crate::font::{Font, F8X13};
use crate::surface::{Button, Input, Mods, Sym};

// ------------------------------------------------------------------ geometry ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub fn contains(&self, (px, py): (i32, i32)) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn inset(&self, by: i32) -> Rect {
        rect(self.x + by, self.y + by, self.w - by * 2, self.h - by * 2)
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Take `n` pixels off the top and return (taken, what is left).
    pub fn cut_top(&self, n: i32) -> (Rect, Rect) {
        let n = n.min(self.h);
        (rect(self.x, self.y, self.w, n), rect(self.x, self.y + n, self.w, self.h - n))
    }
    pub fn cut_bottom(&self, n: i32) -> (Rect, Rect) {
        let n = n.min(self.h);
        (
            rect(self.x, self.bottom() - n, self.w, n),
            rect(self.x, self.y, self.w, self.h - n),
        )
    }
    pub fn cut_left(&self, n: i32) -> (Rect, Rect) {
        let n = n.min(self.w);
        (rect(self.x, self.y, n, self.h), rect(self.x + n, self.y, self.w - n, self.h))
    }
}

// -------------------------------------------------------------------- state ---

/// The small amount that has to survive between frames.
///
/// `active` is the widget a press landed in, and it persists because a press
/// and its release are usually two frames apart. Everything else on the screen
/// is recomputed.
#[derive(Debug, Default)]
pub struct UiState {
    pub pointer: (i32, i32),
    pub down: bool,
    active: Option<u64>,
    pub focus: Option<u64>,
}

/// One frame's worth of input, sorted into the shapes widgets ask about.
#[derive(Debug, Default)]
pub struct Frame {
    presses: Vec<((i32, i32), Button, Mods)>,
    releases: Vec<((i32, i32), Button, Mods)>,
    pub keys: Vec<(Sym, Mods)>,
    /// Every key event, with its scancode and its direction.
    ///
    /// `keys` is what a widget wants: presses, with the modifiers, in a form
    /// that reads like a shortcut. CONTROL WANTS THE OPPOSITE -- the set-1
    /// scancode and whether it went down or up, because that is what travels
    /// on an RDP wire and because a far end that never sees a release has a
    /// key held down for ever. `keymap.rs` produces those numbers, which is
    /// the reason it produces them.
    pub raw_keys: Vec<(u16, Sym, bool, Mods)>,
    pub text: String,
    pub scroll: (i32, i32),
    /// Did the pointer move this frame? A remote desktop only wants to be told
    /// about a position that changed.
    pub motion: bool,
    pub resized: Option<(usize, usize)>,
    pub closed: bool,
}

impl Frame {
    /// Sort a platform's events into a frame, moving the pointer as it goes.
    pub fn gather(events: &[Input], state: &mut UiState) -> Frame {
        let mut f = Frame::default();
        for ev in events {
            match *ev {
                Input::Motion { x, y } => {
                    f.motion = state.pointer != (x, y);
                    state.pointer = (x, y);
                }
                Input::Button { x, y, button, down } => {
                    state.pointer = (x, y);
                    if button == Button::Left {
                        state.down = down;
                    }
                    // The modifiers that matter for a click are the ones held
                    // at the moment of the click, and the platform reports
                    // them with the key events rather than the button ones --
                    // so the frame's current modifier state is used, which is
                    // whatever the last Key event established.
                    let mods = f.keys.last().map(|(_, m)| *m).unwrap_or_default();
                    if down {
                        f.presses.push(((x, y), button, mods));
                    } else {
                        f.releases.push(((x, y), button, mods));
                    }
                }
                Input::Scroll { dx, dy } => {
                    f.scroll.0 += dx;
                    f.scroll.1 += dy;
                }
                Input::Key { scancode, sym, down, mods } => {
                    f.raw_keys.push((scancode, sym, down, mods));
                    if down {
                        f.keys.push((sym, mods));
                    }
                }
                Input::Text(ref s) => f.text.push_str(s),
                Input::Resized { w, h } => f.resized = Some((w, h)),
                Input::Closed => f.closed = true,
                Input::Frame => {}
            }
        }
        f
    }

    /// Was this key pressed this frame, with no modifier held?
    pub fn pressed(&self, sym: Sym) -> bool {
        self.keys.iter().any(|(s, m)| *s == sym && !m.ctrl && !m.logo && !m.alt)
    }

    /// Was this key pressed with the platform's command modifier?
    pub fn chord(&self, sym: Sym) -> bool {
        self.keys.iter().any(|(s, m)| *s == sym && m.toggling())
    }

    /// Every button that went down or up this frame, with where it happened.
    ///
    /// Widgets use `button_hit`, which is press-then-release over the same
    /// rectangle. A remote desktop cannot: it has to forward the down and the
    /// up separately, because the far end is drawing the drag.
    pub fn button_events(&self) -> Vec<((i32, i32), Button, bool)> {
        let mut out: Vec<((i32, i32), Button, bool)> = Vec::new();
        for (at, b, _) in &self.presses {
            out.push((*at, *b, true));
        }
        for (at, b, _) in &self.releases {
            out.push((*at, *b, false));
        }
        out
    }

    /// The modifiers held at the most recent click, for the widget that got it.
    pub fn click_mods(&self) -> Mods {
        self.releases
            .last()
            .map(|(_, _, m)| *m)
            .or_else(|| self.presses.last().map(|(_, _, m)| *m))
            .unwrap_or_default()
    }
}

// ----------------------------------------------------------------------- ui ---

pub struct Ui<'c, 'p> {
    pub c: &'c mut Canvas<'p>,
    pub t: Theme,
    pub f: Frame,
    pub s: &'c mut UiState,
}

/// Widget identity: the rectangle plus a caller-supplied salt.
///
/// Enough when the layout is computed rather than dynamic, which this one is --
/// every rectangle on the screen is derived from the window size and the read
/// model, so two widgets never share both.
fn id_of(r: Rect, salt: &str) -> u64 {
    // FNV-1a. Not a hash anyone is attacking; a hash that is four lines.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    for v in [r.x, r.y, r.w, r.h] {
        for b in v.to_le_bytes() {
            eat(b);
        }
    }
    for b in salt.as_bytes() {
        eat(*b);
    }
    h
}

impl<'c, 'p> Ui<'c, 'p> {
    pub fn begin(c: &'c mut Canvas<'p>, t: Theme, events: &[Input], s: &'c mut UiState) -> Ui<'c, 'p> {
        let f = Frame::gather(events, s);
        Ui { c, t, f, s }
    }

    pub fn pointer(&self) -> (i32, i32) {
        self.s.pointer
    }

    /// The press/release bookkeeping every clickable widget shares.
    ///
    /// Returns true on a release over the same widget the press landed in --
    /// so dragging off a button and letting go does not press it, which is what
    /// every pointer on every platform has meant since the Lisa.
    fn clickable(&mut self, r: Rect, id: u64, enabled: bool) -> bool {
        if !enabled {
            return false;
        }
        for (p, b, _) in &self.f.presses {
            if *b == Button::Left && r.contains(*p) {
                self.s.active = Some(id);
            }
        }
        let mut clicked = false;
        for (p, b, _) in &self.f.releases {
            if *b != Button::Left {
                continue;
            }
            if self.s.active == Some(id) {
                if r.contains(*p) {
                    clicked = true;
                }
                self.s.active = None;
            }
        }
        clicked
    }

    pub fn is_hot(&self, r: Rect) -> bool {
        r.contains(self.s.pointer)
    }

    /// Press-and-release bookkeeping for a widget that draws itself.
    ///
    /// A tile is about this program rather than about widgets -- it knows what
    /// a node is -- so `lab.rs` draws it and asks this whether it was clicked.
    /// The press/release discipline is the button's, so dragging off a tile
    /// and letting go does not select it.
    pub fn button_hit(&mut self, r: Rect) -> bool {
        let id = id_of(r, "\u{0}hit");
        self.clickable(r, id, true)
    }

    // ------------------------------------------------------------ drawing ---

    pub fn panel(&mut self, r: Rect, fill: Rgb) {
        self.c.rect(r.x, r.y, r.w, r.h, fill);
    }

    pub fn outline(&mut self, r: Rect, c: Rgb) {
        self.c.frame(r.x, r.y, r.w, r.h, c);
    }

    /// A 2px frame, for the one thing a 1px frame cannot say clearly.
    ///
    /// Selection is drawn as a frame rather than as a background, because a
    /// selection that is a background colour becomes invisible the moment a
    /// tile also wants to be amber.
    pub fn outline2(&mut self, r: Rect, c: Rgb) {
        self.c.frame(r.x, r.y, r.w, r.h, c);
        self.c.frame(r.x + 1, r.y + 1, r.w - 2, r.h - 2, c);
    }

    pub fn label(&mut self, x: i32, y: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        self.c.text(x, y, s, font, c)
    }

    pub fn label_in(&mut self, x: i32, y: i32, max_w: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        self.c.text_in(x, y, max_w, s, font, c)
    }

    pub fn label_right(&mut self, right: i32, y: i32, s: &str, font: &Font, c: Rgb) -> i32 {
        self.c.text_right(right, y, s, font, c)
    }

    pub fn hrule(&mut self, x: i32, y: i32, len: i32) {
        self.c.hline(x, y, len, self.t.line);
    }

    pub fn vrule(&mut self, x: i32, y: i32, len: i32) {
        self.c.vline(x, y, len, self.t.line);
    }

    // ------------------------------------------------------------ widgets ---

    /// A verb.
    ///
    /// `enabled` exists for the rare case, not the common one: a verb that
    /// cannot apply to the current selection is NOT DRAWN AT ALL. See
    /// docs/console.md -- "absent rather than greyed" is the same rule the web
    /// console follows for the write route, and for the same reason.
    pub fn button(&mut self, r: Rect, label: &str, enabled: bool) -> bool {
        let id = id_of(r, label);
        let clicked = self.clickable(r, id, enabled);
        let hot = enabled && self.is_hot(r);
        let held = enabled && self.s.active == Some(id) && self.s.down;

        let (bg, fg) = if !enabled {
            (self.t.panel, self.t.dim)
        } else if held {
            (self.t.accent, self.t.base)
        } else if hot {
            (self.t.tile, self.t.ink)
        } else {
            (self.t.panel, self.t.ink)
        };
        self.panel(r, bg);
        self.outline(r, if hot || held { self.t.accent } else { self.t.line });

        let tw = F8X13.measure(label) as i32;
        let tx = r.x + (r.w - tw).max(0) / 2;
        let ty = r.y + (r.h - F8X13.height as i32) / 2;
        self.label_in(tx, ty, r.w - 6, label, &F8X13, fg);
        clicked
    }

    /// A row in the left rail: a scene, a tag, a card.
    ///
    /// Returns true when it was clicked, which is how "everything running the
    /// show scene" becomes a selection without typing anything.
    pub fn rail_item(&mut self, r: Rect, label: &str, count: Option<usize>, on: bool) -> bool {
        let id = id_of(r, label);
        let clicked = self.clickable(r, id, true);
        let hot = self.is_hot(r);

        if on {
            self.panel(r, self.t.tile);
        } else if hot {
            self.panel(r, self.t.panel);
        }
        let fg = if on { self.t.ink } else { self.t.dim };
        let ty = r.y + (r.h - F8X13.height as i32) / 2;
        let room = if count.is_some() { r.w - 34 } else { r.w - 10 };
        self.label_in(r.x + 6, ty, room, label, &F8X13, fg);
        if let Some(n) = count {
            self.label_right(r.right() - 6, ty, &n.to_string(), &F8X13, self.t.dim);
        }
        clicked
    }

    /// A status light: filled for up, hollow for missing, half for announced.
    ///
    /// DRAWN RATHER THAN TYPED. `font.rs` bakes five characters past Latin-1
    /// and the half-filled circle the TUI uses is not one of them -- and a
    /// shape is the right answer anyway at tile scale, where a glyph would be
    /// four pixels of soup.
    pub fn light(&mut self, cx: i32, cy: i32, r: i32, state: Light, c: Rgb) {
        match state {
            Light::Full => self.c.disc(cx, cy, r, c),
            Light::Hollow => self.c.ring(cx, cy, r, c),
            Light::Half => {
                self.c.ring(cx, cy, r, c);
                // The left half, filled. Drawn as rows so it needs no clip.
                for dy in -r..=r {
                    let span = ((r * r - dy * dy) as f64).sqrt() as i32;
                    self.c.hline(cx - span, cy + dy, span + 1, c);
                }
            }
        }
    }
}

/// What a status light is saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Full,
    Half,
    Hollow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;
    use crate::surface::Input;

    fn press(x: i32, y: i32) -> Input {
        Input::Button { x, y, button: Button::Left, down: true }
    }
    fn release(x: i32, y: i32) -> Input {
        Input::Button { x, y, button: Button::Left, down: false }
    }

    /// Run one frame against a scratch canvas and hand the closure a `Ui`.
    fn frame<R>(state: &mut UiState, events: &[Input], f: impl FnOnce(&mut Ui) -> R) -> R {
        let mut px = vec![0u8; 200 * 100 * 4];
        let mut c = draw::Canvas::new(&mut px, 200, 100);
        let mut ui = Ui::begin(&mut c, draw::LIGHT, events, state);
        f(&mut ui)
    }

    #[test]
    fn a_rectangle_knows_what_is_inside_it() {
        let r = rect(10, 10, 20, 10);
        assert!(r.contains((10, 10)));
        assert!(r.contains((29, 19)));
        assert!(!r.contains((30, 19)), "the right edge is exclusive");
        assert!(!r.contains((29, 20)), "the bottom edge is exclusive");
        assert!(!r.contains((9, 10)));
    }

    #[test]
    fn cutting_a_rectangle_loses_no_pixels() {
        let r = rect(0, 0, 100, 50);
        let (top, rest) = r.cut_top(12);
        assert_eq!(top, rect(0, 0, 100, 12));
        assert_eq!(rest, rect(0, 12, 100, 38));
        assert_eq!(top.h + rest.h, r.h);

        let (bot, rest) = r.cut_bottom(20);
        assert_eq!(bot, rect(0, 30, 100, 20));
        assert_eq!(rest, rect(0, 0, 100, 30));

        let (left, rest) = r.cut_left(30);
        assert_eq!(left, rect(0, 0, 30, 50));
        assert_eq!(rest, rect(30, 0, 70, 50));
        // A cut larger than the rectangle takes the rectangle, not more.
        assert_eq!(r.cut_top(999).0.h, 50);
    }

    #[test]
    fn a_press_and_a_release_over_the_same_button_is_a_click() {
        let r = rect(10, 10, 60, 20);
        let mut s = UiState::default();
        // Press this frame -- not a click yet. A button that fired on press
        // would fire on a drag that was on its way somewhere else.
        let down = frame(&mut s, &[press(20, 15)], |ui| ui.button(r, "Run", true));
        assert!(!down, "a bare press pressed the button");
        // Release over it, next frame.
        let up = frame(&mut s, &[release(20, 15)], |ui| ui.button(r, "Run", true));
        assert!(up, "press then release did not click");
    }

    #[test]
    fn dragging_off_a_button_and_letting_go_does_not_press_it() {
        let r = rect(10, 10, 60, 20);
        let mut s = UiState::default();
        frame(&mut s, &[press(20, 15)], |ui| ui.button(r, "Power", true));
        let out = frame(&mut s, &[release(150, 80)], |ui| ui.button(r, "Power", true));
        assert!(!out, "released off the button and it fired anyway");
        // And the button is no longer holding the press.
        let after = frame(&mut s, &[release(20, 15)], |ui| ui.button(r, "Power", true));
        assert!(!after, "a stale press survived into the next release");
    }

    #[test]
    fn a_disabled_button_cannot_be_pressed_at_all() {
        let r = rect(10, 10, 60, 20);
        let mut s = UiState::default();
        frame(&mut s, &[press(20, 15)], |ui| ui.button(r, "Control", false));
        let got = frame(&mut s, &[release(20, 15)], |ui| ui.button(r, "Control", false));
        assert!(!got);
    }

    #[test]
    fn a_release_with_no_press_is_not_a_click() {
        // The window gaining focus on a mouse-up is the case this is about.
        let r = rect(10, 10, 60, 20);
        let mut s = UiState::default();
        let got = frame(&mut s, &[release(20, 15)], |ui| ui.button(r, "Scene", true));
        assert!(!got);
    }

    #[test]
    fn two_buttons_in_one_frame_do_not_share_an_identity() {
        let a = rect(10, 10, 60, 20);
        let b = rect(80, 10, 60, 20);
        let mut s = UiState::default();
        frame(&mut s, &[press(20, 15)], |ui| {
            ui.button(a, "Send", true);
            ui.button(b, "Run", true);
        });
        let (hit_a, hit_b) = frame(&mut s, &[release(20, 15)], |ui| {
            (ui.button(a, "Send", true), ui.button(b, "Run", true))
        });
        assert!(hit_a, "the pressed button did not fire");
        assert!(!hit_b, "the neighbour fired too");
    }

    #[test]
    fn motion_moves_the_pointer_and_makes_a_widget_hot() {
        let r = rect(10, 10, 60, 20);
        let mut s = UiState::default();
        let hot = frame(&mut s, &[Input::Motion { x: 20, y: 15 }], |ui| ui.is_hot(r));
        assert!(hot);
        assert_eq!(s.pointer, (20, 15));
        let cold = frame(&mut s, &[Input::Motion { x: 150, y: 90 }], |ui| ui.is_hot(r));
        assert!(!cold);
    }

    #[test]
    fn keys_and_text_are_gathered_separately() {
        // Terminal wants the text, Control wants the scancode, and the
        // interface wants the sym. All three travel.
        let evs = vec![
            Input::Key {
                scancode: 0x1e,
                sym: Sym::Char('a'),
                down: true,
                mods: Mods::default(),
            },
            Input::Text("a".into()),
        ];
        let mut s = UiState::default();
        let (keys, text) = frame(&mut s, &evs, |ui| (ui.f.keys.len(), ui.f.text.clone()));
        assert_eq!(keys, 1);
        assert_eq!(text, "a");
    }

    #[test]
    fn a_key_release_is_not_a_keypress() {
        let mut s = UiState::default();
        let n = frame(
            &mut s,
            &[Input::Key {
                scancode: 0x01,
                sym: Sym::Escape,
                down: false,
                mods: Mods::default(),
            }],
            |ui| ui.f.keys.len(),
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn the_command_chord_is_told_apart_from_the_bare_key() {
        let bare = Input::Key {
            scancode: 0x1e,
            sym: Sym::Char('a'),
            down: true,
            mods: Mods::default(),
        };
        let cmd = Input::Key {
            scancode: 0x1e,
            sym: Sym::Char('a'),
            down: true,
            mods: Mods { logo: true, ..Default::default() },
        };
        let mut s = UiState::default();
        let (p, c) = frame(&mut s, &[bare], |ui| {
            (ui.f.pressed(Sym::Char('a')), ui.f.chord(Sym::Char('a')))
        });
        assert!(p && !c, "a bare key read as a chord");
        let (p, c) = frame(&mut s, &[cmd], |ui| {
            (ui.f.pressed(Sym::Char('a')), ui.f.chord(Sym::Char('a')))
        });
        assert!(!p && c, "select-all read as the letter a");
    }

    #[test]
    fn a_rail_item_reports_its_click() {
        let r = rect(0, 40, 100, 16);
        let mut s = UiState::default();
        frame(&mut s, &[press(30, 45)], |ui| ui.rail_item(r, "show", Some(5), false));
        let got = frame(&mut s, &[release(30, 45)], |ui| {
            ui.rail_item(r, "show", Some(5), false)
        });
        assert!(got);
    }

    #[test]
    fn a_resize_and_a_close_survive_the_gather() {
        let mut s = UiState::default();
        let evs = [Input::Resized { w: 1024, h: 768 }, Input::Closed];
        let f = Frame::gather(&evs, &mut s);
        assert_eq!(f.resized, Some((1024, 768)));
        assert!(f.closed);
    }
}
