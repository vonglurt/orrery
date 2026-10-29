//! Which face of the console is in front of you, and how to get to the others.
//!
//! The interface has more than one job now -- a room of machines, and the pile
//! of SD cards that room was made from -- and a console with two jobs and no
//! way to say which one you are looking at is a console people get lost in.
//!
//! ONE STRIP, IN THE HEADER, ALWAYS VISIBLE. Not a menu, which hides where you
//! are until you open it; not a hamburger, which hides that there is anywhere
//! else at all. The current one lit, in the same place on every view.
//!
//! THE STRIP TELLS A STORY (Paul, 28 October: "rethink about the interface
//! progression. tell a story"; he chose Build, Run, Result). Five chapters,
//! in the order the work goes: a computer is added (Machines), given work
//! (Projects), the work is queued and runs (Queue), what it found is read and
//! promoted (Results), and anything can be watched or touched (Watch). Each
//! chapter's pages are keys in the steel column on the left.

use crate::font::F8X13;
use crate::surface::Sym;
use crate::ui::{rect, Rect, Ui};

/// A face of the console: one page of a chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// A fresh install to a fleet node: probe, the setup recipes, join
    /// (machines.rs).
    Machines,
    /// The cards, images and machines that room was made from.
    Media,
    /// What the fleet works on -- treadcore, crucible -- and which machine
    /// runs which (projects.rs).
    Projects,
    /// The queue: each machine and its job, and the runs (training.rs).
    Queue,
    /// What the queue found: candidates, improvements, the net, flags
    /// (training.rs).
    Results,
    /// The room of machines.
    Lab,
    /// The machines' terminals in a grid, each a resumable tmux session
    /// (sessions.rs).
    Sessions,
}

impl Default for View {
    /// The queue: what is happening now, which is what a person opening the
    /// window most often wants.
    fn default() -> View {
        View::Queue
    }
}

impl View {
    pub fn title(&self) -> &'static str {
        match self {
            View::Machines => "Machines",
            View::Media => "Media",
            View::Projects => "Projects",
            View::Queue => "Queue",
            View::Results => "Results",
            View::Lab => "Lab",
            View::Sessions => "Sessions",
        }
    }

    /// Every view, chapter by chapter.
    pub const ALL: &'static [View] = &[View::Machines, View::Media, View::Projects, View::Queue, View::Results, View::Lab, View::Sessions];

    pub fn chapter(&self) -> Chapter {
        match self {
            View::Machines | View::Media => Chapter::Machines,
            View::Projects => Chapter::Projects,
            View::Queue => Chapter::Queue,
            View::Results => Chapter::Results,
            View::Lab | View::Sessions => Chapter::Watch,
        }
    }
}

/// A chapter of the story: a key in the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chapter {
    Machines,
    Projects,
    Queue,
    Results,
    Watch,
}

impl Chapter {
    pub const ALL: &'static [Chapter] = &[Chapter::Machines, Chapter::Projects, Chapter::Queue, Chapter::Results, Chapter::Watch];

    pub fn title(&self) -> &'static str {
        match self {
            Chapter::Machines => "Machines",
            Chapter::Projects => "Projects",
            Chapter::Queue => "Queue",
            Chapter::Results => "Results",
            Chapter::Watch => "Watch",
        }
    }

    /// Its views, the first where the strip goes.
    pub fn views(&self) -> &'static [View] {
        match self {
            Chapter::Machines => &[View::Machines, View::Media],
            Chapter::Projects => &[View::Projects],
            Chapter::Queue => &[View::Queue],
            Chapter::Results => &[View::Results],
            Chapter::Watch => &[View::Lab, View::Sessions],
        }
    }

    /// The chapter in a few words, cut into the foot of the column.
    pub fn story(&self) -> &'static [&'static str] {
        match self {
            Chapter::Machines => &["1. Add a", "computer: probe", "it, set it up,", "join it."],
            Chapter::Projects => &["2. Give it work:", "treadcore on the", "CPUs, crucible", "on the GPUs."],
            Chapter::Queue => &["3. The work runs:", "each machine's", "job, and the", "runs asked for."],
            Chapter::Results => &["4. What it found:", "candidates", "promote", "themselves."],
            Chapter::Watch => &["5. Look in: the", "room, and every", "terminal."],
        }
    }
}

/// The steel column's width, left of every view's body.
pub const COLUMN_W: i32 = 150;

/// A page key in the column: its word, whether it is the one shown, and its
/// lamp's colour when lit (None: dark).
pub struct Key {
    pub label: String,
    pub on: bool,
    pub lamp: Option<crate::draw::Rgb>,
}

/// The column's keys, top down, and the chapter's story at its foot.
/// Returns the key clicked.
pub fn column(ui: &mut Ui, at: Rect, keys: &[Key], chapter: Chapter) -> Option<usize> {
    let t = ui.t;
    if !t.metal {
        ui.panel(at, t.panel);
        ui.vrule(at.right() - 1, at.y, at.h);
    }
    let mut clicked = None;
    let mut y = at.y + 4;
    for (i, k) in keys.iter().enumerate() {
        let r = rect(at.x + 6, y, at.w - 12, 26);
        if t.metal {
            let f = if k.on { crate::draw::Finish::Pressed } else { crate::draw::Finish::Button };
            ui.c.metal(r.x, r.y, r.w, r.h, f, 7);
            let lamp = if k.on { k.lamp.unwrap_or(t.up) } else { k.lamp.unwrap_or(0x3a3e40) };
            ui.c.disc(r.x + 11, r.y + r.h / 2, 4, lamp);
            let ty = r.y + (r.h - F8X13.height as i32) / 2 + if k.on { 1 } else { 0 };
            ui.label_in(r.x + 21, ty, r.w - 25, &k.label, &F8X13, t.engrave);
        } else {
            if k.on {
                ui.panel(r, t.tile);
                ui.c.rect(r.x, r.y, 2, r.h, t.accent);
            }
            let ty = r.y + (r.h - F8X13.height as i32) / 2;
            ui.label_in(r.x + 8, ty, r.w - 12, &k.label, &F8X13, if k.on { t.ink } else { k.lamp.unwrap_or(t.dim) });
        }
        if ui.button_hit(r) && !k.on {
            clicked = Some(i);
        }
        y += 30;
    }
    let lines = chapter.story();
    let mut sy = at.bottom() - 8 - lines.len() as i32 * (F8X13.height as i32 + 2);
    for l in lines {
        ui.label_in(at.x + 8, sy, at.w - 12, l, &F8X13, if t.metal { 0x3a3e42 } else { t.dim });
        sy += F8X13.height as i32 + 2;
    }
    clicked
}

/// The chapter's views as column keys, the current one on.
pub fn view_keys(current: View) -> Vec<Key> {
    current.chapter().views().iter().map(|v| Key { label: v.title().to_string(), on: *v == current, lamp: None }).collect()
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
    let here = current.chapter();

    for (i, c) in Chapter::ALL.iter().enumerate() {
        let w = tab_w(*c, t.metal);
        let tab = rect(x, at.y, w, at.h);
        let on = *c == here;
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
            ui.label_in(nx + 6, ty, key.right() - nx - 8, c.title(), &F8X13, t.engrave);
            if hot && !on {
                ui.c.rect(key.x + 8, key.bottom() - 3, key.w - 16, 1, t.accent);
            }
        } else {
            if on {
                ui.panel(tab, t.tile);
                // A lit underline rather than a filled box: a filled tab in
                // the header reads as a button that does something.
                ui.c.rect(tab.x, tab.bottom() - 2, tab.w, 2, t.accent);
            } else if hot {
                ui.panel(tab, t.panel);
            }
            let ty = tab.y + (tab.h - F8X13.height as i32) / 2;
            ui.label_in(tab.x + 11, ty, w - 16, c.title(), &F8X13, if on { t.ink } else { t.dim });
        }
        if ui.button_hit(tab) && !on {
            chosen = Some(c.views()[0]);
        }
        x += w;
    }
    chosen
}

/// How wide the strip is in a theme, so a header can lay out around it.
pub fn width_for(metal: bool) -> i32 {
    Chapter::ALL.iter().map(|c| tab_w(*c, metal)).sum()
}

/// A tab's width: the word, and in metal a lamp and the digit as well.
fn tab_w(c: Chapter, metal: bool) -> i32 {
    F8X13.measure(c.title()) as i32 + if metal { 44 } else { 22 }
}

/// Every view's header height.
pub const HEADER_H: i32 = 40;

/// The frame every view shares (28 October, the metal redesign): the
/// header with the name, the chapters and `right` in a readout; the steel
/// column with the chapter's page keys (`keys`, or its views when None); and
/// in the metal theme the window's steel round a black well. Returns the
/// body, a view chosen (a chapter, a view key, or Command and a digit when
/// `chords`), and a page key clicked when `keys` were given.
pub fn header(ui: &mut Ui, current: View, chords: bool, right: &str, keys: Option<&[Key]>) -> (Rect, Option<View>, Option<usize>) {
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
    let (col, rest) = rest.cut_left(COLUMN_W);
    let mut page = None;
    match keys {
        Some(k) => page = column(ui, col, k, current.chapter()),
        None => {
            let k = view_keys(current);
            if let Some(i) = column(ui, col, &k, current.chapter()) {
                go = Some(current.chapter().views()[i]);
            }
        }
    }
    let body = if t.metal {
        let b = rect(rest.x, rest.y, rest.w - 6, rest.h - 6);
        ui.c.well(b.x, b.y, b.w, b.h, t.base, 8);
        b
    } else {
        rest
    };
    (body, go, page)
}

/// The keyboard's way there: the platform's command modifier and a digit.
///
/// Command-1 on a Mac, Control-1 on a node -- `Mods::toggling` is the same
/// gesture spelled two ways, which is why the interface asks for the gesture
/// rather than for a key.
pub fn chord(ui: &Ui) -> Option<View> {
    for (i, c) in Chapter::ALL.iter().enumerate() {
        let digit = char::from_digit(i as u32 + 1, 10)?;
        if ui.f.chord(Sym::Char(digit)) {
            return Some(c.views()[0]);
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
        let sum: i32 = Chapter::ALL
            .iter()
            .map(|c| F8X13.measure(c.title()) as i32 + 22)
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
        // The second chapter, Projects, starts after Machines' width.
        let first_w = F8X13.measure("Machines") as i32 + 22;

        let got = frame(&mut s, &click(first_w + 10, 12), |ui| {
            strip(ui, bar, View::Lab)
        });
        assert_eq!(got, Some(View::Projects));

        // Re-entering the view you are already in must not report a change --
        // a caller that reloaded on every click would throw away a half-typed
        // field every time somebody clicked the tab they were on.
        let mut s = UiState::default();
        let got = frame(&mut s, &click(10, 12), |ui| strip(ui, bar, View::Media));
        assert_eq!(got, None, "Media is in the Machines chapter, the one clicked");
    }

    #[test]
    fn a_click_that_misses_every_tab_changes_nothing() {
        let bar = rect(0, 0, width_for(false), 24);
        let mut s = UiState::default();
        let got = frame(&mut s, &click(380, 50), |ui| strip(ui, bar, View::Lab));
        assert_eq!(got, None);
    }

    #[test]
    fn the_command_chord_reaches_each_chapter_by_its_position() {
        for (i, c) in Chapter::ALL.iter().enumerate() {
            let want = &c.views()[0];
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
        // The chord is a digit, so more than nine chapters needs another idea.
        assert!(Chapter::ALL.len() <= 9, "the command-digit chords have run out");
    }

    #[test]
    fn a_chapter_story_fits_its_column() {
        let room = (COLUMN_W - 12) as usize / F8X13.width as usize;
        for c in Chapter::ALL {
            for l in c.story() {
                assert!(l.chars().count() <= room, "{:?}: {:?} is wider than {} letters", c, l, room);
            }
        }
    }

    #[test]
    fn every_view_is_in_exactly_one_chapter_and_the_chapters_are_the_story() {
        for v in View::ALL {
            let n = Chapter::ALL.iter().filter(|c| c.views().contains(v)).count();
            assert_eq!(n, 1, "{:?} is in {} chapters", v, n);
            assert!(v.chapter().views().contains(v));
        }
        let titles: Vec<&str> = Chapter::ALL.iter().map(|c| c.title()).collect();
        assert_eq!(titles, ["Machines", "Projects", "Queue", "Results", "Watch"]);
    }
}
