//! Text, key and button mapping to HID usages.
//!
//! Keyboard: HID usage page 0x07, US layout. Upper-case letters and shifted
//! symbols add Left Shift. Limits: only printable US-ASCII, `\n` and `\t`
//! map to keys. Other characters (accents, non-Latin scripts, emoji) have no
//! key on a US layout. [`text_to_strokes`] reports them as skipped. If the
//! iPhone uses a hardware-keyboard layout other than US, symbols can come out
//! different (for example on a German layout `y` and `z` swap).
//!
//! Buttons: HID Consumer page 0x0C, values from idevice `tools/src/hid.rs`.

use ap_core::{Button, Error, Key, Modifiers, Result};

/// HID usage page for keyboards.
pub const PAGE_KEYBOARD: u64 = 0x07;
/// HID usage page "Consumer" (hardware buttons).
pub const PAGE_CONSUMER: u64 = 0x0C;

pub const KEY_LEFT_CTRL: u64 = 0xE0;
pub const KEY_LEFT_SHIFT: u64 = 0xE1;
pub const KEY_LEFT_ALT: u64 = 0xE2;
pub const KEY_LEFT_GUI: u64 = 0xE3;

/// One key stroke: modifiers held down, then the key pressed and released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    /// Modifier usages in press order (released in reverse).
    pub modifiers: Vec<u64>,
    /// Main key usage.
    pub usage: u64,
}

/// A hardware button press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonPress {
    pub page: u64,
    pub usage: u64,
    /// How long to hold the button, in milliseconds.
    pub hold_ms: u64,
}

/// US layout: char -> (usage, needs Shift).
pub fn char_usage(c: char) -> Option<(u64, bool)> {
    Some(match c {
        'a'..='z' => (0x04 + (c as u64 - 'a' as u64), false),
        'A'..='Z' => (0x04 + (c as u64 - 'A' as u64), true),
        '1'..='9' => (0x1E + (c as u64 - '1' as u64), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        '-' => (0x2D, false),
        '_' => (0x2D, true),
        '=' => (0x2E, false),
        '+' => (0x2E, true),
        '[' => (0x2F, false),
        '{' => (0x2F, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        _ => return None,
    })
}

/// Usage of a named (non-character) key.
fn named_usage(k: &Key) -> Option<u64> {
    Some(match k {
        Key::Enter => 0x28,
        Key::Escape => 0x29,
        Key::Backspace => 0x2A,
        Key::Tab => 0x2B,
        Key::Space => 0x2C,
        Key::Delete => 0x4C,
        Key::Right => 0x4F,
        Key::Left => 0x50,
        Key::Down => 0x51,
        Key::Up => 0x52,
        _ => return None,
    })
}

fn modifier_usages(m: Modifiers) -> Vec<u64> {
    let mut v = Vec::new();
    if m.meta {
        v.push(KEY_LEFT_GUI);
    }
    if m.ctrl {
        v.push(KEY_LEFT_CTRL);
    }
    if m.alt {
        v.push(KEY_LEFT_ALT);
    }
    if m.shift {
        v.push(KEY_LEFT_SHIFT);
    }
    v
}

/// Map a [`Key`] (with any modifiers) to one stroke.
///
/// `cmd+h` (home), `cmd+space` (Spotlight) and `cmd+a` work on iOS with a
/// hardware keyboard. Errors for characters with no US key.
pub fn key_stroke(key: &Key) -> Result<Stroke> {
    let (mods, base) = key.parts();
    let mut modifiers = modifier_usages(mods);
    let usage = match base {
        Key::Char(c) => {
            let (u, shift) = char_usage(*c).ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "key {c:?} has no key on a US keyboard (only US-ASCII is supported)"
                ))
            })?;
            // Ctrl/Cmd combos use the base key: `cmd+A` is the same as `cmd+a`.
            let only_shift_case = c.is_ascii_uppercase() && !mods.is_empty() && !mods.shift;
            if shift && !modifiers.contains(&KEY_LEFT_SHIFT) && !only_shift_case {
                modifiers.push(KEY_LEFT_SHIFT);
            }
            u
        }
        other => named_usage(other)
            .ok_or_else(|| Error::InvalidArgument(format!("unsupported key {other:?}")))?,
    };
    Ok(Stroke { modifiers, usage })
}

/// Map text to strokes. Returns the strokes and the characters that have no
/// US-ASCII key (skipped). `\r\n` and `\r` count as Enter.
pub fn text_to_strokes(text: &str) -> (Vec<Stroke>, Vec<char>) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut strokes = Vec::with_capacity(normalized.len());
    let mut skipped = Vec::new();
    for c in normalized.chars() {
        match char_usage(c) {
            Some((usage, shift)) => strokes.push(Stroke {
                modifiers: if shift { vec![KEY_LEFT_SHIFT] } else { vec![] },
                usage,
            }),
            None => skipped.push(c),
        }
    }
    (strokes, skipped)
}

pub const HOME: ButtonPress = ButtonPress {
    page: PAGE_CONSUMER,
    usage: 0x40,
    hold_ms: 80,
};
pub const LOCK: ButtonPress = ButtonPress {
    page: PAGE_CONSUMER,
    usage: 0x30,
    hold_ms: 200,
};
pub const VOLUME_UP: ButtonPress = ButtonPress {
    page: PAGE_CONSUMER,
    usage: 0xE9,
    hold_ms: 80,
};
pub const VOLUME_DOWN: ButtonPress = ButtonPress {
    page: PAGE_CONSUMER,
    usage: 0xEA,
    hold_ms: 80,
};
pub const SIRI: ButtonPress = ButtonPress {
    page: PAGE_CONSUMER,
    usage: 0xCF,
    hold_ms: 1200,
};

/// Gap between the two Home presses that open the App Switcher.
pub const DOUBLE_HOME_GAP_MS: u64 = 120;

/// Button presses for a [`Button`]. `AppSwitcher` is a double Home press.
/// `Back` has no button: it is empty here, and the HID backend does a back
/// edge swipe instead (see `hid.rs`).
pub fn button_presses(b: Button) -> Vec<ButtonPress> {
    match b {
        Button::Home => vec![HOME],
        Button::Lock => vec![LOCK],
        Button::VolumeUp => vec![VOLUME_UP],
        Button::VolumeDown => vec![VOLUME_DOWN],
        Button::Siri => vec![SIRI],
        Button::AppSwitcher => vec![HOME, HOME],
        Button::Back => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chars() {
        assert_eq!(char_usage('a'), Some((0x04, false)));
        assert_eq!(char_usage('Z'), Some((0x1D, true)));
        assert_eq!(char_usage('0'), Some((0x27, false)));
        assert_eq!(char_usage('?'), Some((0x38, true)));
        assert_eq!(char_usage('é'), None);
    }

    #[test]
    fn text() {
        let (s, skipped) = text_to_strokes("Hi!\r\nü");
        assert_eq!(skipped, vec!['ü']);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].modifiers, vec![KEY_LEFT_SHIFT]);
        assert_eq!(s[0].usage, 0x0B);
        assert!(s[1].modifiers.is_empty());
        assert_eq!(s[2].modifiers, vec![KEY_LEFT_SHIFT]);
        assert_eq!(s[3].usage, 0x28);
    }

    #[test]
    fn keys_and_combos() {
        assert_eq!(key_stroke(&Key::Enter).unwrap().usage, 0x28);
        assert_eq!(key_stroke(&Key::Delete).unwrap().usage, 0x4C);
        let s = key_stroke(&Key::parse("cmd+space").unwrap()).unwrap();
        assert_eq!(s.modifiers, vec![KEY_LEFT_GUI]);
        assert_eq!(s.usage, 0x2C);
        let s = key_stroke(&Key::parse("cmd+shift+left").unwrap()).unwrap();
        assert_eq!(s.modifiers, vec![KEY_LEFT_GUI, KEY_LEFT_SHIFT]);
        assert_eq!(s.usage, 0x50);
        let s = key_stroke(&Key::Char('?')).unwrap();
        assert_eq!(s.modifiers, vec![KEY_LEFT_SHIFT]);
        let s = key_stroke(&Key::parse("cmd+A").unwrap()).unwrap();
        assert_eq!(s.modifiers, vec![KEY_LEFT_GUI]);
        assert_eq!(s.usage, 0x04);
        assert!(key_stroke(&Key::Char('ç')).is_err());
    }

    #[test]
    fn buttons() {
        assert_eq!(button_presses(Button::Home), vec![HOME]);
        assert_eq!(button_presses(Button::AppSwitcher).len(), 2);
        assert!(button_presses(Button::Back).is_empty());
        assert_eq!(button_presses(Button::Lock)[0].usage, 0x30);
        assert_eq!(button_presses(Button::Siri)[0].hold_ms, 1200);
    }
}
