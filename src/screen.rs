//! Control: a node's desktop in a pane, and the operator's hands on it.
//!
//! THE SAME SHAPE AS THE OTHER TWO PANES. A shell is a `media::Job`; a file
//! browser is a `files::Files`; this is a `Screen`. Each owns its connection,
//! each is drawn by one function, and each is in front of everything else
//! while it is open -- because a pane that takes every keystroke cannot have
//! the lab's own keys live underneath it.
//!
//! ONE SESSION AT A TIME, WHICH IS A DESIGN DECISION AND NOT A LIMITATION.
//! `docs/console.md` §III-B priced eight live desktops on 512 MB boards and
//! refused them; the verb table says Control is `Arity::One` for the same
//! reason. What the lab shows for eight machines is eight tiles, not eight
//! screens.
//!
//! NEAREST-NEIGHBOUR SCALING, LETTERBOXED. A node's desktop is 1024x768 and
//! the pane is whatever the window is; smoothing it would mean a filter and a
//! second buffer for something nobody is reading text in. The pointer is
//! mapped back through the same arithmetic, so a click lands where it looks
//! like it lands.

use crate::draw::Theme;
use crate::font::F8X13;
use crate::rdp;
use crate::surface::{Button, Sym};
use crate::ui::{rect, Rect, Ui};

const HEAD: i32 = 26;

/// What the operator did to the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Stay,
    Close,
}

pub struct Screen {
    pub node: String,
    pub session: rdp::Session,
    pub note: String,
    /// Where the desktop was drawn last frame, so the pointer can be mapped
    /// back through exactly the arithmetic that drew it.
    at: Rect,
    /// Which buttons the far end currently believes are held.
    buttons: u8,
}

impl Screen {
    pub fn open(node: String, session: rdp::Session) -> Screen {
        let note = match session.cert.as_ref() {
            Some(c) => format!("{} — {}x{} — {}", node, session.width, session.height, c.subject),
            None => format!("{} — {}x{}", node, session.width, session.height),
        };
        Screen {
            node,
            session,
            note,
            at: rect(0, 0, 1, 1),
            buttons: 0,
        }
    }

    /// Take whatever has arrived. Called once a frame, like every other pane.
    pub fn pump(&mut self) {
        if let Err(e) = self.session.poll() {
            self.note = e;
        }
    }
}

/// Draw the desktop and forward what the operator did to it.
pub fn draw(ui: &mut Ui, at: Rect, s: &mut Screen) -> Pane {
    let t = ui.t;
    ui.panel(at, t.base);
    let (head, body) = at.cut_top(HEAD);
    ui.panel(head, t.panel);
    ui.hrule(head.x, head.bottom() - 1, head.w);
    ui.label_in(head.x + 10, head.y + 6, head.w - 200, &s.note, &F8X13, t.ink);
    let close = rect(head.right() - 78, head.y + 3, 68, 20);
    let closed = ui.button(close, "Close", true);

    // The largest whole-pixel rectangle of the right shape that fits.
    let (dw, dh) = (s.session.width as i32, s.session.height as i32);
    if dw <= 0 || dh <= 0 {
        return Pane::Stay;
    }
    let scale_num = std::cmp::min(body.w * 1000 / dw, body.h * 1000 / dh).max(1);
    let w = (dw * scale_num / 1000).max(1);
    let h = (dh * scale_num / 1000).max(1);
    let x0 = body.x + (body.w - w) / 2;
    let y0 = body.y + (body.h - h) / 2;
    s.at = rect(x0, y0, w, h);

    // The desktop itself. One pass, nearest neighbour, no allocation.
    let frame = &s.session.frame;
    for y in 0..h {
        let sy = (y as i64 * dh as i64 / h as i64) as usize;
        for x in 0..w {
            let sx = (x as i64 * dw as i64 / w as i64) as usize;
            // The framebuffer is already `0x00RRGGBB`, which is what the
            // canvas wants -- that is why `rdp.rs` converts on arrival rather
            // than here, once per pixel per frame.
            ui.c.set(x0 + x, y0 + y, frame[sy * dw as usize + sx]);
        }
    }

    let quit = forward(ui, s);
    if closed || quit {
        return Pane::Close;
    }
    Pane::Stay
}

/// Send the frame's input to the node. Returns true when the operator asked to
/// leave rather than to type.
fn forward(ui: &mut Ui, s: &mut Screen) -> bool {
    // ESCAPE IS THE ONE KEY THIS PANE KEEPS. Everything else goes to the far
    // end, including the platform's own shortcuts -- which is the point of
    // Control and the reason the pane is in front of everything.
    if ui.f.raw_keys.iter().any(|(_, sym, down, m)| {
        *sym == Sym::Escape && *down && (m.ctrl || m.logo || m.alt)
    }) {
        return true;
    }

    for (scancode, _sym, down, _mods) in ui.f.raw_keys.clone() {
        if scancode == 0 {
            // A key the platform could not give a scancode for. Sending zero
            // would press a key the far end does not have.
            continue;
        }
        if let Err(e) = s.session.key(scancode, down) {
            s.note = e;
            return false;
        }
    }

    let (px, py) = ui.pointer();
    let inside = s.at.contains((px, py));
    let (x, y) = if inside {
        (
            ((px - s.at.x) as i64 * s.session.width as i64 / s.at.w.max(1) as i64) as u16,
            ((py - s.at.y) as i64 * s.session.height as i64 / s.at.h.max(1) as i64) as u16,
        )
    } else {
        (0, 0)
    };

    let mut changed = false;
    for ((bx, by), button, down) in ui.f.button_events() {
        if !s.at.contains((bx, by)) {
            continue;
        }
        let bit = match button {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
        };
        if down {
            s.buttons |= bit;
        } else {
            s.buttons &= !bit;
        }
        changed = true;
    }

    let scrolled = ui.f.scroll.1;
    if inside && (changed || scrolled != 0 || moved(ui)) {
        let wheel = scrolled.clamp(-3, 3) as i8;
        if let Err(e) = s.session.pointer(x, y, s.buttons, wheel) {
            s.note = e;
        }
    }
    false
}

/// Did the pointer move this frame? A desktop that is sent its own position
/// sixty times a second wastes the link it is trying to draw through.
fn moved(ui: &Ui) -> bool {
    ui.f.motion
}

/// The theme is read through `Ui`, and this keeps the import honest.
#[allow(dead_code)]
fn _unused(_t: Theme) {}
