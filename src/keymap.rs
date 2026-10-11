//! US keyboard layout used by `varmint type` and `varmint key`.

use crate::virtio::input::keys::*;

/// Key code and whether shift is held for a printable character.
pub fn char_key(c: char) -> Option<(u16, bool)> {
    if c.is_ascii_uppercase() {
        return char_key(c.to_ascii_lowercase()).map(|(code, _)| (code, true));
    }

    Some(match c {
        'a' => (KEY_A, false),
        'b' => (KEY_B, false),
        'c' => (KEY_C, false),
        'd' => (KEY_D, false),
        'e' => (KEY_E, false),
        'f' => (KEY_F, false),
        'g' => (KEY_G, false),
        'h' => (KEY_H, false),
        'i' => (KEY_I, false),
        'j' => (KEY_J, false),
        'k' => (KEY_K, false),
        'l' => (KEY_L, false),
        'm' => (KEY_M, false),
        'n' => (KEY_N, false),
        'o' => (KEY_O, false),
        'p' => (KEY_P, false),
        'q' => (KEY_Q, false),
        'r' => (KEY_R, false),
        's' => (KEY_S, false),
        't' => (KEY_T, false),
        'u' => (KEY_U, false),
        'v' => (KEY_V, false),
        'w' => (KEY_W, false),
        'x' => (KEY_X, false),
        'y' => (KEY_Y, false),
        'z' => (KEY_Z, false),
        '1' => (KEY_1, false),
        '2' => (KEY_2, false),
        '3' => (KEY_3, false),
        '4' => (KEY_4, false),
        '5' => (KEY_5, false),
        '6' => (KEY_6, false),
        '7' => (KEY_7, false),
        '8' => (KEY_8, false),
        '9' => (KEY_9, false),
        '0' => (KEY_0, false),
        '!' => (KEY_1, true),
        '@' => (KEY_2, true),
        '#' => (KEY_3, true),
        '$' => (KEY_4, true),
        '%' => (KEY_5, true),
        '^' => (KEY_6, true),
        '&' => (KEY_7, true),
        '*' => (KEY_8, true),
        '(' => (KEY_9, true),
        ')' => (KEY_0, true),
        '-' => (KEY_MINUS, false),
        '_' => (KEY_MINUS, true),
        '=' => (KEY_EQUAL, false),
        '+' => (KEY_EQUAL, true),
        '[' => (KEY_LEFTBRACE, false),
        '{' => (KEY_LEFTBRACE, true),
        ']' => (KEY_RIGHTBRACE, false),
        '}' => (KEY_RIGHTBRACE, true),
        '\\' => (KEY_BACKSLASH, false),
        '|' => (KEY_BACKSLASH, true),
        ';' => (KEY_SEMICOLON, false),
        ':' => (KEY_SEMICOLON, true),
        '\'' => (KEY_APOSTROPHE, false),
        '"' => (KEY_APOSTROPHE, true),
        '`' => (KEY_GRAVE, false),
        '~' => (KEY_GRAVE, true),
        ',' => (KEY_COMMA, false),
        '<' => (KEY_COMMA, true),
        '.' => (KEY_DOT, false),
        '>' => (KEY_DOT, true),
        '/' => (KEY_SLASH, false),
        '?' => (KEY_SLASH, true),
        ' ' => (KEY_SPACE, false),
        '\n' => (KEY_ENTER, false),
        '\t' => (KEY_TAB, false),
        _ => return None,
    })
}

/// Key code for a key name such as `enter`, `ctrl` or `f5`, or a single character.
pub fn named_key(name: &str) -> Option<u16> {
    let name = name.to_ascii_lowercase();
    Some(match name.as_str() {
        "ctrl" | "control" => KEY_LEFTCTRL,
        "shift" => KEY_LEFTSHIFT,
        "alt" => KEY_LEFTALT,
        "super" | "meta" | "win" | "cmd" => KEY_LEFTMETA,
        "enter" | "return" => KEY_ENTER,
        "esc" | "escape" => KEY_ESC,
        "tab" => KEY_TAB,
        "space" => KEY_SPACE,
        "backspace" => KEY_BACKSPACE,
        "delete" | "del" => KEY_DELETE,
        "insert" => KEY_INSERT,
        "home" => KEY_HOME,
        "end" => KEY_END,
        "pageup" => KEY_PAGEUP,
        "pagedown" => KEY_PAGEDOWN,
        "up" => KEY_UP,
        "down" => KEY_DOWN,
        "left" => KEY_LEFT,
        "right" => KEY_RIGHT,
        "f1" => KEY_F1,
        "f2" => KEY_F2,
        "f3" => KEY_F3,
        "f4" => KEY_F4,
        "f5" => KEY_F5,
        "f6" => KEY_F6,
        "f7" => KEY_F7,
        "f8" => KEY_F8,
        "f9" => KEY_F9,
        "f10" => KEY_F10,
        "f11" => KEY_F11,
        "f12" => KEY_F12,
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            return char_key(c).map(|(code, _)| code);
        }
    })
}
