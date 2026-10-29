//! Which face of the console is in front of you, and how to get to the others.
//!
//! The interface has more than one job now -- a room of machines, and the pile
//! of SD cards that room was made from -- and a console with two jobs and no
//! way to say which one you are looking at is a console people get lost in.
//!
//! ONE STRIP, IN THE HEADER, ALWAYS VISIBLE. Not a menu, which hides where you
//! are until you open it; not a hamburger, which hides that there is anywhere
//! else at all. Two or three words, the current one lit, in the same place on
//! every view. The whole of the navigation is legible without clicking
//! anything, which at eight tiles and three views is the correct amount of
//! navigation to build.

use crate::font::F8X13;
use crate::surface::Sym;
use crate::ui::{rect, Rect, Ui};

/// A face of the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The room of machines.
    Lab,
    /// The cards, images and machines that room was made from.
    Media,
    /// A fresh install to a fleet node: probe, the setup recipes, join
    /// (machines.rs).
    Machines,
    /// What the fleet is training, and which machines are doing it
    /// (training.rs).
    Training,
    /// The machines' terminals in a grid, each a resumable tmux session
    /// (sessions.rs).
    Sessions,
}

impl Default for View {
    fn default() -> View {
        View::Lab
    }
}

impl View {
    pub fn title(&self) -> &'static str {
        match self {
            View::Lab => "Lab",
            View::Media => "Media",
            View::Machines => "Machines",
            View::Training => "Training",
            View::Sessions => "Sessions",
        }
    }

    /// Every view, in the order the strip draws them.
    pub const ALL: &'static [View] = &[View::Lab, View::Media, View::Machines, View::Training, View::Sessions];
}

/// A header's text on the right, in the room the strip leaves: right-aligned
/// when it fits, clipped from the left of that room when it does not. Five
/// views made the strip wide enough for the Sessions hint to run over it
/// (28 October).
pub fn header_right(ui: &mut Ui, header: Rect, pad: i32, y: i32, text: &str) {
    let t = ui.t;
    let left = pad + crate::font::F10X20.measure("orrery") as i32 + 14 + width_for(t.metal) + 16;
    let right = header.right() - pad;
    let w = F8X13.measure(text) as i32;
    if t.metal {
        // treadcore's top strip: white letters in a black readout.
        if right - left < 40 {
            return;
        }
        let lw = (w + 20).min(right - left);
        let well = rect(right - lw, header.y + 7, lw, header.h - 14);
        ui.c.well(well.x, well.y, well.w, well.h, t.base, 6);
        ui.label_in(well.x + 10, y, well.w - 16, text, &F8X13, t.ink);
        return;
    }
    if right - w >= left {
        ui.label_right(right, y, text, &F8X13, t.dim);
    } else if right > left {
        ui.label_in(left, y, right - left, text, &F8X13, t.dim);
    }
}

/// Draw the strip and report a click on a view that is not the current one.
///
/// Returns `None` when nothing was chosen, INCLUDING when the current view was
/// clicked -- re-entering the view you are already in should not reset it, and
/// a caller that reloaded on every click would throw away a half-typed field
/// every time somebody clicked the tab they were already on.
pub fn strip(ui: &mut Ui, at: Rect, current: View) -> Option<View> {
    let t = ui.t;
    let mut x = at.x;
    let mut chosen = None;

    for (i, v) in View::ALL.iter().enumerate() {
        let w = tab_w(*v, t.metal);
        let tab = rect(x, at.y, w, at.h);
        let on = *v == current;
        let hot = ui.is_hot(tab);

        if t.metal {
            // A key in treadcore's order column: its digit (Command and it
            // go there), its word, and the one that is lit pressed in.
            let key = rect(tab.x + 2, tab.y + 6, tab.w - 4, tab.h - 12);
            let f = if on { crate::draw::Finish::Pressed } else { crate::draw::Finish::Button };
            ui.c.metal(key.x, key.y, key.w, key.h, f, 7);
            let ty = key.y + (key.h - F8X13.height as i32) / 2 + if on { 1 } else { 0 };
            let lamp = if on { t.up } else { 0x3a3e40 };
            ui.c.disc(key.x + 11, key.y + key.h / 2, 4, lamp);
            let digit = char::from_digit(i as u32 + 1, 10).unwrap_or(' ').to_string();
            let nx = ui.label(key.x + 20, ty, &digit, &F8X13, 0x4a4e52);
            ui.label_in(nx + 6, ty, key.right() - nx - 8, v.title(), &F8X13, t.engrave);
            if hot && !on {
                ui.c.rect(key.x + 8, key.bottom() - 3, key.w - 16, 1, t.accent);
            }
            if ui.button_hit(tab) && !on {
                chosen = Some(*v);
            }
            x += w;
            continue;
        }

        if on {
            ui.panel(tab, t.tile);
            // A lit underline rather than a filled box: the strip sits in the
            // header beside the fleet's name, and a filled tab there reads as
            // a button that does something to the fleet.
            ui.c.rect(tab.x, tab.bottom() - 2, tab.w, 2, t.accent);
        } else if hot {
            ui.panel(tab, t.panel);
        }

        let ty = tab.y + (tab.h - F8X13.height as i32) / 2;
        let fg = if on { t.ink } else { t.dim };
        ui.label_in(tab.x + 11, ty, w - 16, v.title(), &F8X13, fg);

        if ui.button_hit(tab) && !on {
            chosen = Some(*v);
        }
        x += w;
    }
    chosen
}

/// How wide the strip is in a theme, so a header can lay out around it.
pub fn width_for(metal: bool) -> i32 {
    View::ALL.iter().map(|v| tab_w(*v, metal)).sum()
}

/// A tab's width: the word, and in metal a lamp and the digit as well.
fn tab_w(v: View, metal: bool) -> i32 {
    F8X13.measure(v.title()) as i32 + if metal { 44 } else { 22 }
}

/// Every view's header height.
pub const HEADER_H: i32 = 40;

/// The frame every view shares (28 October, the metal redesign): the
/// header with the name, the strip and `right` in a readout, and in the
/// metal theme the window's steel round a black well. Returns the body
/// under the header and a view chosen by a click or (when `chords`) by
/// Command and a digit.
pub fn header(ui: &mut Ui, current: View, chords: bool, right: &str) -> (Rect, Option<View>) {
    let t = ui.t;
    let (w, h) = (ui.c.w as i32, ui.c.h as i32);
    let pad = 10;
    let head_h = HEADER_H;
    let (header, rest) = rect(0, 0, w, h).cut_top(head_h);
    if t.metal {
        ui.c.metal(0, 0, w, h, crate::draw::Finish::Plate, 0);
    } else {
        ui.panel(rect(0, 0, w, h), t.base);
        ui.panel(header, t.panel);
        ui.hrule(header.x, header.bottom() - 1, header.w);
    }
    let ty = header.y + (head_h - crate::font::F10X20.height as i32) / 2;
    ui.label(pad, ty, "orrery", &crate::font::F10X20, if t.metal { t.engrave } else { t.ink });
    let nav_at = rect(pad + crate::font::F10X20.measure("orrery") as i32 + 14, header.y, width_for(t.metal), head_h);
    let mut go = strip(ui, nav_at, current);
    if chords {
        if let Some(v) = chord(ui) {
            go = Some(v);
        }
    }
    if !right.is_empty() {
        let sy = header.y + (head_h - F8X13.height as i32) / 2;
        header_right(ui, header, pad, sy, right);
    }
    let body = if t.metal {
        let b = rect(rest.x + 6, rest.y, rest.w - 12, rest.h - 6);
        ui.c.well(b.x, b.y, b.w, b.h, t.base, 8);
        b
    } else {
        rest
    };
    (body, go)
}

/// The keyboard's way there: the platform's command modifier and a digit.
///
/// Command-1 on a Mac, Control-1 on a node -- `Mods::toggling` is the same
/// gesture spelled two ways, which is why the interface asks for the gesture
/// rather than for a key.
pub fn chord(ui: &Ui) -> Option<View> {
    for (i, v) in View::ALL.iter().enumerate() {
        let digit = char::from_digit(i as u32 + 1, 10)?;
        if ui.f.chord(Sym::Char(digit)) {
            return Some(*v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;
    use crate::surface::{Button, Input, Mods};
    use crate::ui::UiState;

    fn frame<R>(state: &mut UiState, events: &[Input], f: impl FnOnce(&mut Ui) -> R) -> R {
        let mut px = vec![0u8; 400 * 60 * 4];
        let mut c = draw::Canvas::new(&mut px, 400, 60);
        let mut ui = Ui::begin(&mut c, draw::LIGHT, events, state);
        f(&mut ui)
    }

    fn click(x: i32, y: i32) -> Vec<Input> {
        vec![
            Input::Button { x, y, button: Button::Left, down: true },
            Input::Button { x, y, button: Button::Left, down: false },
        ]
    }

    #[test]
    fn the_strip_is_as_wide_as_the_tabs_it_draws() {
        // A header lays out around this, so it has to be true before anything
        // is drawn rather than discovered after.
        let sum: i32 = View::ALL
            .iter()
            .map(|v| F8X13.measure(v.title()) as i32 + 22)
            .sum();
        assert_eq!(width_for(false), sum);
        // Five views since Machines (28 October): about 370 pixels, still a
        // strip beside the fleet's name at 960 wide; in metal, with a lamp
        // and a digit on each key, under 500.
        assert!(width_for(false) > 0 && width_for(false) < 400, "the strip is not a strip");
        assert!(width_for(true) < 500, "the metal strip is {}", width_for(true));
    }

    #[test]
    fn clicking_the_other_tab_moves_and_clicking_the_current_one_does_not() {
        let bar = rect(0, 0, width_for(false), 24);
        let mut s = UiState::default();
        // The second tab. Media starts after Lab's width.
        let lab_w = F8X13.measure("Lab") as i32 + 22;

        let got = frame(&mut s, &click(lab_w + 10, 12), |ui| {
            strip(ui, bar, View::Lab)
        });
        assert_eq!(got, Some(View::Media));

        // Re-entering the view you are already in must not report a change --
        // a caller that reloaded on every click would throw away a half-typed
        // field every time somebody clicked the tab they were on.
        let mut s = UiState::default();
        let got = frame(&mut s, &click(10, 12), |ui| strip(ui, bar, View::Lab));
        assert_eq!(got, None);
    }

    #[test]
    fn a_click_that_misses_every_tab_changes_nothing() {
        let bar = rect(0, 0, width_for(false), 24);
        let mut s = UiState::default();
        let got = frame(&mut s, &click(380, 50), |ui| strip(ui, bar, View::Lab));
        assert_eq!(got, None);
    }

    #[test]
    fn the_command_chord_reaches_each_view_by_its_position() {
        for (i, want) in View::ALL.iter().enumerate() {
            let digit = char::from_digit(i as u32 + 1, 10).unwrap();
            let ev = [Input::Key {
                scancode: 0,
                sym: Sym::Char(digit),
                down: true,
                mods: Mods { logo: true, ..Default::default() },
            }];
            let mut s = UiState::default();
            let got = frame(&mut s, &ev, |ui| chord(ui));
            assert_eq!(got, Some(*want), "command-{} did not reach {:?}", digit, want);
        }
    }

    #[test]
    fn a_bare_digit_is_not_navigation() {
        // Typing 1 into a field must not move the console to another view.
        let ev = [Input::Key {
            scancode: 0,
            sym: Sym::Char('1'),
            down: true,
            mods: Mods::default(),
        }];
        let mut s = UiState::default();
        assert_eq!(frame(&mut s, &ev, |ui| chord(ui)), None);
    }

    #[test]
    fn every_view_has_a_name_and_no_two_share_one() {
        for (i, v) in View::ALL.iter().enumerate() {
            assert!(!v.title().is_empty());
            for other in View::ALL.iter().skip(i + 1) {
                assert_ne!(v.title(), other.title());
                assert_ne!(v, other);
            }
        }
        // The chord is a digit, so more than nine views needs another idea.
        assert!(View::ALL.len() <= 9, "the command-digit chords have run out");
    }
}
