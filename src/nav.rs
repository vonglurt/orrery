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
        }
    }

    /// Every view, in the order the strip draws them.
    pub const ALL: &'static [View] = &[View::Lab, View::Media];
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

    for v in View::ALL {
        let w = F8X13.measure(v.title()) as i32 + 22;
        let tab = rect(x, at.y, w, at.h);
        let on = *v == current;
        let hot = ui.is_hot(tab);

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

/// How wide the strip will be, so a header can lay out around it.
pub fn width() -> i32 {
    View::ALL
        .iter()
        .map(|v| F8X13.measure(v.title()) as i32 + 22)
        .sum()
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
        assert_eq!(width(), sum);
        assert!(width() > 0 && width() < 300, "the strip is not a strip");
    }

    #[test]
    fn clicking_the_other_tab_moves_and_clicking_the_current_one_does_not() {
        let bar = rect(0, 0, width(), 24);
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
        let bar = rect(0, 0, width(), 24);
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
