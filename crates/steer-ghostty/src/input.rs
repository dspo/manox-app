//! GPUI keystrokes → libghostty-vt key events.

use crate::sys::key_consts::*;
use crate::sys::{GHOSTTY_MODS_ALT, GHOSTTY_MODS_CTRL, GHOSTTY_MODS_SHIFT, GHOSTTY_MODS_SUPER};
use gpui::Modifiers;

/// Map a GPUI `Keystroke.key` name to the physical `GhosttyKey` const, plus
/// the text to hand the encoder (`key_char` — the layout-produced character).
pub fn map_key(gpui_key: &str) -> c_int_key {
    match gpui_key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "space" => GHOSTTY_KEY_SPACE,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "backquote" | "`" => GHOSTTY_KEY_BACKQUOTE,
        "-" | "minus" => GHOSTTY_KEY_MINUS,
        "=" | "equal" => GHOSTTY_KEY_EQUAL,
        "[" | "bracketleft" => GHOSTTY_KEY_BRACKET_LEFT,
        "]" | "bracketright" => GHOSTTY_KEY_BRACKET_RIGHT,
        "\\" => GHOSTTY_KEY_BACKSLASH,
        ";" | "semicolon" => GHOSTTY_KEY_SEMICOLON,
        "'" | "quote" => GHOSTTY_KEY_QUOTE,
        "," => GHOSTTY_KEY_COMMA,
        "." => GHOSTTY_KEY_PERIOD,
        "/" => GHOSTTY_KEY_SLASH,
        "capslock" => GHOSTTY_KEY_CAPS_LOCK,
        f if f.len() >= 2 && f.starts_with('f') => {
            // f1..f25 are sequential constants in the header.
            match f[1..].parse::<u32>() {
                Ok(n @ 1..=25) => GHOSTTY_KEY_F1 + (n as i32 - 1),
                _ => GHOSTTY_KEY_UNIDENTIFIED,
            }
        }
        single => {
            let mut chars = single.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_uppercase() {
                    return GHOSTTY_KEY_A + (c as i32 - b'A' as i32);
                }
                if c.is_ascii_lowercase() {
                    return GHOSTTY_KEY_A + (c as i32 - b'a' as i32);
                }
                if c.is_ascii_digit() {
                    return GHOSTTY_KEY_DIGIT_0 + (c as i32 - b'0' as i32);
                }
            }
            GHOSTTY_KEY_UNIDENTIFIED
        }
    }
}

/// `GhosttyKey` as a plain i32 alias for readability above.
#[allow(non_camel_case_types)]
type c_int_key = std::ffi::c_int;

pub fn ghostty_mods(m: &Modifiers) -> u16 {
    let mut out = 0;
    if m.shift {
        out |= GHOSTTY_MODS_SHIFT;
    }
    if m.control {
        out |= GHOSTTY_MODS_CTRL;
    }
    if m.alt {
        out |= GHOSTTY_MODS_ALT;
    }
    if m.platform {
        out |= GHOSTTY_MODS_SUPER;
    }
    out
}
