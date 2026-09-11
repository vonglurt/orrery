//! The keyboard, baked in.
//!
//! `font.rs` carries two X11 fonts because a node has no font package. The same
//! argument applies one layer over: **a node has no `libxkbcommon`.**
//!
//! `wl_keyboard.keymap` hands a client a file descriptor holding an `XKB_V1`
//! text keymap, and the correct thing to do with it is give it to
//! `libxkbcommon`. That is a crate fetch, or an `apk add`, on the one machine
//! this is meant to run on -- so this does what `font.rs` did with the font and
//! bakes a table in.
//!
//! THE CONSEQUENCE, STATED PLAINLY: on Linux, a non-US layout types the wrong
//! letters and a compose key does nothing. `us` is the only value that works
//! and is the default. The alternative is `libxkbcommon` -- a dependency -- or
//! an xkb parser, which is a larger and much less interesting program than the
//! one this is. It is recorded here and in `docs/surface.md` §5 rather than
//! discovered later.
//!
//! macOS needs none of this. `NSEvent.characters` is already composed and
//! carries the whole input-method stack for free, which is the one place that
//! platform is easier than this one.
//!
//! ---
//!
//! **THE HAPPY ACCIDENT THAT MAKES THIS SMALL.** Linux's evdev keycodes for the
//! main keyboard were taken from the PC/AT set-1 scancodes, so for everything
//! from `KEY_ESC` to `KEY_KPDOT` the two are *the same number*. A table mapping
//! one to the other would be eighty-three identity rows. What actually needs a
//! table is the extended block -- the arrows, the navigation cluster, the right
//! modifiers -- which set 1 reaches with an `0xE0` prefix byte and evdev
//! numbers straight on from 96.

use crate::surface::Sym;

/// Wayland's `wl_keyboard.key` carries the evdev code plus this.
///
/// The offset is X11's, inherited by the Wayland protocol, and forgetting it
/// shifts the entire keyboard by one key -- which reads as a broken layout
/// rather than as an off-by-eight.
pub const WL_KEYCODE_OFFSET: u16 = 8;

/// The set-1 scancodes that need an `0xE0` prefix, stored with it.
///
/// RDP's fastpath input carries a byte plus a `KBDFLAGS_EXTENDED` flag, so the
/// high byte here is what sets that flag. Storing the arrows as bare `0x48`
/// and friends would send the NUMPAD keys instead -- the same physical
/// scancodes, which is exactly why the extended prefix exists.
const EXTENDED: &[(u16, u16)] = &[
    (96, 0xe01c),  // KP enter
    (97, 0xe01d),  // right ctrl
    (98, 0xe035),  // KP slash
    (100, 0xe038), // right alt
    (102, 0xe047), // home
    (103, 0xe048), // up
    (104, 0xe049), // page up
    (105, 0xe04b), // left
    (106, 0xe04d), // right
    (107, 0xe04f), // end
    (108, 0xe050), // down
    (109, 0xe051), // page down
    (110, 0xe052), // insert
    (111, 0xe053), // delete
    (125, 0xe05b), // left meta
    (126, 0xe05c), // right meta
];

/// evdev keycode to PC/AT set-1 scancode, `0xE0`-prefixed where it needs it.
///
/// Returns 0 for a key with no set-1 equivalent, and 0 is "do not send" rather
/// than a key: Control forwarding the wrong scancode is worse than Control
/// forwarding none.
pub fn set1(evdev: u16) -> u16 {
    if let Some((_, sc)) = EXTENDED.iter().find(|(k, _)| *k == evdev) {
        return *sc;
    }
    // The identity range. 84 and 85..86 are gaps on a US keyboard; 87 and 88
    // are F11 and F12, which land on 0x57 and 0x58 and are inside it.
    match evdev {
        1..=83 | 87 | 88 => evdev,
        _ => 0,
    }
}

pub fn is_extended(scancode: u16) -> bool {
    scancode & 0xff00 == 0xe000
}

/// The named keys. Everything else is a character.
const NAMED: &[(u16, Sym)] = &[
    (1, Sym::Escape),
    (14, Sym::Backspace),
    (15, Sym::Tab),
    (28, Sym::Return),
    (57, Sym::Space),
    (59, Sym::Func(1)),
    (60, Sym::Func(2)),
    (61, Sym::Func(3)),
    (62, Sym::Func(4)),
    (63, Sym::Func(5)),
    (64, Sym::Func(6)),
    (65, Sym::Func(7)),
    (66, Sym::Func(8)),
    (67, Sym::Func(9)),
    (68, Sym::Func(10)),
    (87, Sym::Func(11)),
    (88, Sym::Func(12)),
    (96, Sym::Return), // KP enter is Return to the interface
    (102, Sym::Home),
    (103, Sym::Up),
    (104, Sym::PageUp),
    (105, Sym::Left),
    (106, Sym::Right),
    (107, Sym::End),
    (108, Sym::Down),
    (109, Sym::PageDown),
    (111, Sym::Delete),
];

/// The US layout's printable keys: evdev code, unshifted, shifted.
///
/// Generated once from `/usr/share/X11/xkb/symbols/us` and committed, the way
/// `font.rs` is committed, because `copal-build` on a node must not need an X
/// package to compile a console.
const US: &[(u16, char, char)] = &[
    (2, '1', '!'), (3, '2', '@'), (4, '3', '#'), (5, '4', '$'), (6, '5', '%'),
    (7, '6', '^'), (8, '7', '&'), (9, '8', '*'), (10, '9', '('), (11, '0', ')'),
    (12, '-', '_'), (13, '=', '+'),
    (16, 'q', 'Q'), (17, 'w', 'W'), (18, 'e', 'E'), (19, 'r', 'R'), (20, 't', 'T'),
    (21, 'y', 'Y'), (22, 'u', 'U'), (23, 'i', 'I'), (24, 'o', 'O'), (25, 'p', 'P'),
    (26, '[', '{'), (27, ']', '}'),
    (30, 'a', 'A'), (31, 's', 'S'), (32, 'd', 'D'), (33, 'f', 'F'), (34, 'g', 'G'),
    (35, 'h', 'H'), (36, 'j', 'J'), (37, 'k', 'K'), (38, 'l', 'L'),
    (39, ';', ':'), (40, '\'', '"'), (41, '`', '~'), (43, '\\', '|'),
    (44, 'z', 'Z'), (45, 'x', 'X'), (46, 'c', 'C'), (47, 'v', 'V'), (48, 'b', 'B'),
    (49, 'n', 'N'), (50, 'm', 'M'),
    (51, ',', '<'), (52, '.', '>'), (53, '/', '?'),
];

/// What this key means to the interface.
///
/// `Sym::Char` is always the UNSHIFTED letter, because the interface binds
/// physical keys -- `c` is Control whether or not shift is down, and a binding
/// table that had to carry both cases would be two tables.
pub fn sym(evdev: u16) -> Sym {
    if let Some((_, s)) = NAMED.iter().find(|(k, _)| *k == evdev) {
        return *s;
    }
    match US.iter().find(|(k, _, _)| *k == evdev) {
        Some((_, lower, _)) => Sym::Char(*lower),
        None => Sym::Unknown,
    }
}

/// What this key means as text, when it means any.
///
/// `None` for every key that is not a character, which is what keeps the
/// arrows and the function keys out of a Terminal's input.
pub fn text(evdev: u16, shift: bool, caps: bool) -> Option<char> {
    if evdev == 57 {
        return Some(' ');
    }
    let (_, lower, upper) = US.iter().find(|(k, _, _)| *k == evdev)?;
    // Caps lock is a LETTER lock, not a shift lock: it capitalises `a` and
    // leaves `1` alone, which is why this is not `shift ^ caps` throughout.
    if lower.is_ascii_alphabetic() {
        Some(if shift ^ caps { *upper } else { *lower })
    } else if shift {
        Some(*upper)
    } else {
        Some(*lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_keyboard_is_the_identity_because_evdev_took_set_ones_numbers() {
        // The accident this module rests on. If it is ever not true, the
        // eighty-three identity rows have to come back as a table.
        for (evdev, set1_code) in [
            (1u16, 0x01u16), // esc
            (2, 0x02),       // 1
            (13, 0x0d),      // =
            (14, 0x0e),      // backspace
            (15, 0x0f),      // tab
            (16, 0x10),      // q
            (28, 0x1c),      // enter
            (29, 0x1d),      // left ctrl
            (30, 0x1e),      // a
            (42, 0x2a),      // left shift
            (44, 0x2c),      // z
            (57, 0x39),      // space
            (59, 0x3b),      // F1
            (68, 0x44),      // F10
            (83, 0x53),      // KP dot
            (87, 0x57),      // F11
            (88, 0x58),      // F12
        ] {
            assert_eq!(set1(evdev), set1_code, "evdev {} is not set-1 {:#x}", evdev, set1_code);
        }
    }

    #[test]
    fn the_arrows_carry_the_extended_prefix_and_not_the_numpad() {
        // 0x48 alone is numpad 8. The arrows are the SAME scancodes with an
        // 0xE0 prefix, which is the entire reason the prefix exists -- so a
        // bare 0x48 would move the far end's cursor with the number pad.
        assert_eq!(set1(103), 0xe048, "up");
        assert_eq!(set1(105), 0xe04b, "left");
        assert_eq!(set1(106), 0xe04d, "right");
        assert_eq!(set1(108), 0xe050, "down");
        assert_eq!(set1(111), 0xe053, "delete");
        for evdev in [102, 103, 104, 105, 106, 107, 108, 109, 110, 111] {
            assert!(is_extended(set1(evdev)), "evdev {} lost its prefix", evdev);
        }
        // And the main block is not extended.
        for evdev in [1, 16, 28, 30, 57, 88] {
            assert!(!is_extended(set1(evdev)), "evdev {} gained a prefix", evdev);
        }
    }

    #[test]
    fn a_key_with_no_set_one_equivalent_sends_nothing_rather_than_something() {
        // Control forwarding the wrong scancode is worse than forwarding none.
        for evdev in [0, 84, 85, 86, 190, 240, 700] {
            assert_eq!(set1(evdev), 0, "evdev {} invented a scancode", evdev);
        }
    }

    #[test]
    fn the_wayland_offset_is_applied_once_and_in_one_direction() {
        // Forgetting it shifts the whole keyboard by one key, which reads as a
        // broken layout rather than as an off-by-eight.
        assert_eq!(WL_KEYCODE_OFFSET, 8);
        let wl_escape = 1 + WL_KEYCODE_OFFSET;
        assert_eq!(sym(wl_escape - WL_KEYCODE_OFFSET), Sym::Escape);
        assert_ne!(sym(wl_escape), Sym::Escape, "the offset was not applied");
    }

    #[test]
    fn a_letter_is_its_unshifted_self_to_the_interface() {
        // The interface binds physical keys: `c` is Control whether or not
        // shift is down, so a binding table never carries both cases.
        assert_eq!(sym(46), Sym::Char('c'));
        assert_eq!(sym(30), Sym::Char('a'));
        assert_eq!(sym(19), Sym::Char('r'));
        // And the named keys win over the character table.
        assert_eq!(sym(57), Sym::Space);
        assert_eq!(sym(28), Sym::Return);
        assert_eq!(sym(1), Sym::Escape);
        assert_eq!(sym(103), Sym::Up);
        assert_eq!(sym(999), Sym::Unknown);
    }

    #[test]
    fn shift_and_caps_lock_are_not_the_same_key() {
        // Caps lock is a LETTER lock. It capitalises `a` and leaves `1` alone.
        assert_eq!(text(30, false, false), Some('a'));
        assert_eq!(text(30, true, false), Some('A'));
        assert_eq!(text(30, false, true), Some('A'));
        assert_eq!(text(30, true, true), Some('a'), "shift with caps did not cancel");

        assert_eq!(text(2, false, false), Some('1'));
        assert_eq!(text(2, true, false), Some('!'));
        assert_eq!(text(2, false, true), Some('1'), "caps lock shifted a digit");
        assert_eq!(text(2, true, true), Some('!'));
    }

    #[test]
    fn nothing_that_is_not_a_character_becomes_text() {
        // This is what keeps the arrows out of a Terminal's input.
        for evdev in [1, 14, 15, 28, 59, 88, 102, 103, 105, 108, 111, 29, 42] {
            assert_eq!(text(evdev, false, false), None, "evdev {} became text", evdev);
        }
        assert_eq!(text(57, false, false), Some(' '), "space is text");
    }

    #[test]
    fn the_us_table_has_no_duplicate_key_and_no_duplicate_glyph() {
        for (i, (code, lo, up)) in US.iter().enumerate() {
            for (other, olo, oup) in US.iter().skip(i + 1) {
                assert_ne!(code, other, "two rows for evdev {}", code);
                assert_ne!(lo, olo, "two keys produce {:?}", lo);
                assert_ne!(up, oup, "two keys produce {:?}", up);
            }
        }
        // Every printable key also has a scancode, or Control cannot send it.
        for (code, lo, _) in US {
            assert_ne!(set1(*code), 0, "{:?} has no scancode", lo);
        }
    }

    #[test]
    fn every_named_key_that_is_not_a_duplicate_has_its_own_scancode() {
        for (i, (code, s)) in NAMED.iter().enumerate() {
            for (other, os) in NAMED.iter().skip(i + 1) {
                assert_ne!(code, other, "two rows for evdev {}", code);
                // KP enter and Return are deliberately the same Sym; nothing
                // else may be.
                if s == os {
                    assert!(
                        matches!(s, Sym::Return),
                        "{:?} is reachable from two keys",
                        s
                    );
                }
            }
            assert_ne!(set1(*code), 0, "{:?} has no scancode", s);
        }
    }
}
