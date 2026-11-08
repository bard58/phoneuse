//! HID report descriptor and input reports, byte-for-byte the same as upstream
//! `hid_daemon.py`.
//!
//! | id | report | payload |
//! |----|--------|---------|
//! | 1 | boot-style keyboard | `mods, 0, k1..k6` (8 bytes) |
//! | 2 | relative mouse | `buttons, dx, dy, wheel` (4 bytes, i8 -127..127) |
//! | 3 | absolute pointer | `buttons, x_lo, x_hi, y_lo, y_hi` (5 bytes, 0..32767) |
//! | 4 | consumer control | `eject` (1 byte, bit 0) |
//!
//! The firmware in `firmware/` uses the same descriptor ([`REPORT_MAP`]).

use ap_core::{Error, Key, Modifiers, Result};

/// Keyboard report id.
pub const ID_KEYBOARD: u8 = 1;
/// Relative mouse report id.
pub const ID_MOUSE: u8 = 2;
/// Absolute pointer report id.
pub const ID_ABSOLUTE: u8 = 3;
/// Consumer control (Eject) report id.
pub const ID_CONSUMER: u8 = 4;

/// Largest absolute coordinate (logical maximum of report 3).
pub const ABS_MAX: u16 = 32767;

/// The HID report descriptor (report map). Same bytes as upstream `REPORT_MAP`.
pub const REPORT_MAP: &[u8] = &[
    // Report 1: keyboard (modifiers, reserved, 6 keys)
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x03, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xc0,
    // Report 2: relative mouse (3 buttons, X, Y, wheel; -127..127)
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
    0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08,
    0x95, 0x03, 0x81, 0x06, 0xc0, 0xc0,
    // Report 3: absolute pointer (3 buttons, X and Y 0..32767)
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x03, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
    0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95,
    0x02, 0x81, 0x02, 0xc0, 0xc0,
    // Report 4: consumer control, one bit: Eject (toggles the on-screen keyboard)
    0x05, 0x0c, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x04, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01,
    0x09, 0xb8, 0x81, 0x02, 0x95, 0x07, 0x81, 0x03, 0xc0,
];

/// Keyboard modifier bits (byte 0 of report 1).
pub mod mods {
    pub const CTRL: u8 = 0x01;
    pub const SHIFT: u8 = 0x02;
    pub const ALT: u8 = 0x04;
    /// Command on iOS.
    pub const GUI: u8 = 0x08;
}

/// Keyboard usage codes used by the gesture model.
pub mod usage {
    pub const H: u8 = 0x0b;
    pub const ENTER: u8 = 40;
    pub const ESCAPE: u8 = 41;
    pub const BACKSPACE: u8 = 42;
    pub const TAB: u8 = 43;
    pub const SPACE: u8 = 44;
    pub const DELETE: u8 = 76;
    pub const RIGHT: u8 = 79;
    pub const LEFT: u8 = 80;
    pub const DOWN: u8 = 81;
    pub const UP: u8 = 82;
}

/// One input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Report 1: modifier bits and up to 6 held keys.
    Keyboard { mods: u8, keys: [u8; 6] },
    /// Report 2: relative motion. `buttons` bit 0 = left.
    Mouse {
        buttons: u8,
        dx: i8,
        dy: i8,
        wheel: i8,
    },
    /// Report 3: absolute position, 0..=[`ABS_MAX`] on both axes.
    Absolute { buttons: u8, x: u16, y: u16 },
    /// Report 4: Eject held (`true`) or released.
    Consumer { eject: bool },
}

impl Report {
    /// All keys up.
    pub const KEYS_UP: Report = Report::Keyboard {
        mods: 0,
        keys: [0; 6],
    };

    /// One key with modifiers.
    pub fn key(usage: u8, mods: u8) -> Report {
        Report::Keyboard {
            mods,
            keys: [usage, 0, 0, 0, 0, 0],
        }
    }

    /// Relative motion with no buttons (the drag reports).
    pub fn rel(dx: i32, dy: i32) -> Report {
        Report::Mouse {
            buttons: 0,
            dx: dx.clamp(-127, 127) as i8,
            dy: dy.clamp(-127, 127) as i8,
            wheel: 0,
        }
    }

    /// Absolute report at screen fractions `(fx, fy)`, clamped to 0..1.
    /// Same conversion as upstream: `round(f * 32767)`.
    pub fn abs(fx: f64, fy: f64, pressed: bool) -> Report {
        Report::Absolute {
            buttons: u8::from(pressed),
            x: abs_coord(fx),
            y: abs_coord(fy),
        }
    }

    /// The report id.
    pub fn id(&self) -> u8 {
        match self {
            Report::Keyboard { .. } => ID_KEYBOARD,
            Report::Mouse { .. } => ID_MOUSE,
            Report::Absolute { .. } => ID_ABSOLUTE,
            Report::Consumer { .. } => ID_CONSUMER,
        }
    }

    /// Payload bytes without the report id.
    pub fn payload(&self) -> Vec<u8> {
        match *self {
            Report::Keyboard { mods, keys } => {
                let mut v = vec![mods, 0];
                v.extend_from_slice(&keys);
                v
            }
            Report::Mouse {
                buttons,
                dx,
                dy,
                wheel,
            } => vec![buttons & 0x07, dx as u8, dy as u8, wheel as u8],
            Report::Absolute { buttons, x, y } => {
                let x = x.min(ABS_MAX);
                let y = y.min(ABS_MAX);
                vec![
                    buttons & 0x07,
                    (x & 0xff) as u8,
                    (x >> 8) as u8,
                    (y & 0xff) as u8,
                    (y >> 8) as u8,
                ]
            }
            Report::Consumer { eject } => vec![u8::from(eject)],
        }
    }

    /// Report id followed by the payload (USB HID wire form, and the body of a
    /// protocol `REPORT` message).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = vec![self.id()];
        v.extend(self.payload());
        v
    }

    /// Parse `id, payload...` back into a report.
    pub fn from_bytes(b: &[u8]) -> Result<Report> {
        let bad = || Error::protocol(format!("bad report bytes {b:02x?}"));
        let (&id, p) = b.split_first().ok_or_else(bad)?;
        Ok(match (id, p.len()) {
            (ID_KEYBOARD, 8) => Report::Keyboard {
                mods: p[0],
                keys: [p[2], p[3], p[4], p[5], p[6], p[7]],
            },
            (ID_MOUSE, 4) => Report::Mouse {
                buttons: p[0],
                dx: p[1] as i8,
                dy: p[2] as i8,
                wheel: p[3] as i8,
            },
            (ID_ABSOLUTE, 5) => Report::Absolute {
                buttons: p[0],
                x: u16::from_le_bytes([p[1], p[2]]),
                y: u16::from_le_bytes([p[3], p[4]]),
            },
            (ID_CONSUMER, 1) => Report::Consumer {
                eject: p[0] & 1 != 0,
            },
            _ => return Err(bad()),
        })
    }
}

/// Screen fraction to absolute coordinate.
pub fn abs_coord(f: f64) -> u16 {
    let f = if f.is_nan() { 0.0 } else { f.clamp(0.0, 1.0) };
    (f * f64::from(ABS_MAX)).round() as u16
}

/// Absolute coordinate back to a screen fraction.
pub fn abs_fraction(v: u16) -> f64 {
    f64::from(v.min(ABS_MAX)) / f64::from(ABS_MAX)
}

/// US-layout usage code and modifier for a character (upstream `CHARS`).
pub fn char_usage(c: char) -> Option<(u8, u8)> {
    const SHIFT: u8 = mods::SHIFT;
    Some(match c {
        'a'..='z' => (4 + (c as u8 - b'a'), 0),
        'A'..='Z' => (4 + (c as u8 - b'A'), SHIFT),
        '1'..='9' => (30 + (c as u8 - b'1'), 0),
        '0' => (39, 0),
        '!' => (30, SHIFT),
        '@' => (31, SHIFT),
        '#' => (32, SHIFT),
        '$' => (33, SHIFT),
        '%' => (34, SHIFT),
        '^' => (35, SHIFT),
        '&' => (36, SHIFT),
        '*' => (37, SHIFT),
        '(' => (38, SHIFT),
        ')' => (39, SHIFT),
        '-' => (45, 0),
        '_' => (45, SHIFT),
        '=' => (46, 0),
        '+' => (46, SHIFT),
        '[' => (47, 0),
        '{' => (47, SHIFT),
        ']' => (48, 0),
        '}' => (48, SHIFT),
        '\\' => (49, 0),
        '|' => (49, SHIFT),
        ';' => (51, 0),
        ':' => (51, SHIFT),
        '\'' => (52, 0),
        '"' => (52, SHIFT),
        '`' => (53, 0),
        '~' => (53, SHIFT),
        ',' => (54, 0),
        '<' => (54, SHIFT),
        '.' => (55, 0),
        '>' => (55, SHIFT),
        '/' => (56, 0),
        '?' => (56, SHIFT),
        ' ' => (usage::SPACE, 0),
        '\n' => (usage::ENTER, 0),
        '\t' => (usage::TAB, 0),
        _ => return None,
    })
}

/// Modifier bits for ap-core [`Modifiers`].
pub fn modifier_bits(m: Modifiers) -> u8 {
    let mut b = 0;
    if m.ctrl {
        b |= mods::CTRL;
    }
    if m.shift {
        b |= mods::SHIFT;
    }
    if m.alt {
        b |= mods::ALT;
    }
    if m.meta {
        b |= mods::GUI;
    }
    b
}

/// Usage code and modifier bits for an ap-core [`Key`] (including combos).
pub fn key_usage(key: &Key) -> Result<(u8, u8)> {
    let (m, base) = key.parts();
    let mut bits = modifier_bits(m);
    let code = match base {
        Key::Enter => usage::ENTER,
        Key::Backspace => usage::BACKSPACE,
        Key::Delete => usage::DELETE,
        Key::Tab => usage::TAB,
        Key::Escape => usage::ESCAPE,
        Key::Space => usage::SPACE,
        Key::Up => usage::UP,
        Key::Down => usage::DOWN,
        Key::Left => usage::LEFT,
        Key::Right => usage::RIGHT,
        Key::Char(c) => {
            let (code, shift) = char_usage(*c).ok_or_else(|| {
                Error::InvalidArgument(format!("cannot send key {c:?} (US layout only)"))
            })?;
            bits |= shift;
            code
        }
        Key::Combo { .. } => unreachable!("Key::parts returns the base key"),
    };
    Ok((code, bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The upstream descriptor, as the hex string in hid_daemon.py.
    const UPSTREAM_HEX: &str = concat!(
        "05010906a1018501050719e029e71500250175019508810295017508810395067508150025650507190029658100c0",
        "05010902a10185020901a1000509190129031500250195037501810295017505810305010930093109381581257f750895038106c0c0",
        "05010902a10185030901a1000509190129031500250195037501810295017505810305010930093115002",
        "6ff7f751095028102c0c0",
        "050c0901a10185041500250175019501",
        "09b88102",
        "95078103c0",
    );

    #[test]
    fn report_map_matches_upstream() {
        let hex: String = REPORT_MAP.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, UPSTREAM_HEX);
    }

    #[test]
    fn report_bytes() {
        assert_eq!(
            Report::key(usage::H, mods::GUI).to_bytes(),
            [1, 0x08, 0, 0x0b, 0, 0, 0, 0, 0]
        );
        assert_eq!(Report::rel(-3, 200).to_bytes(), [2, 0, 0xfd, 0x7f, 0]);
        assert_eq!(
            Report::abs(1.0, 0.0, true).to_bytes(),
            [3, 1, 0xff, 0x7f, 0, 0]
        );
        // 0.5 * 32767 = 16383.5 -> 16384 = 0x4000
        assert_eq!(
            Report::abs(0.5, 2.0, false).to_bytes(),
            [3, 0, 0x00, 0x40, 0xff, 0x7f]
        );
        assert_eq!(Report::Consumer { eject: true }.to_bytes(), [4, 1]);
        for r in [
            Report::KEYS_UP,
            Report::rel(5, -5),
            Report::abs(0.25, 0.75, true),
            Report::Consumer { eject: false },
        ] {
            assert_eq!(Report::from_bytes(&r.to_bytes()).unwrap(), r);
        }
        assert!(Report::from_bytes(&[9, 1]).is_err());
    }

    #[test]
    fn keys() {
        assert_eq!(char_usage('a'), Some((4, 0)));
        assert_eq!(char_usage('Z'), Some((29, mods::SHIFT)));
        assert_eq!(char_usage('0'), Some((39, 0)));
        assert_eq!(char_usage('?'), Some((56, mods::SHIFT)));
        assert_eq!(char_usage('é'), None);
        let k = Key::parse("cmd+space").unwrap();
        assert_eq!(key_usage(&k).unwrap(), (usage::SPACE, mods::GUI));
        let k = Key::parse("ctrl+A").unwrap();
        assert_eq!(key_usage(&k).unwrap(), (4, mods::CTRL | mods::SHIFT));
    }
}
