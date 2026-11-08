//! Input vocabulary: hardware buttons, keys, gestures.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Point;

/// Physical or system buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    Siri,
    AppSwitcher,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::Home,
        Button::Lock,
        Button::VolumeUp,
        Button::VolumeDown,
        Button::Siri,
        Button::AppSwitcher,
    ];

    /// Parse names like `home`, `volume-up`, `VolumeUp`, `power`.
    pub fn parse(s: &str) -> Option<Self> {
        let n: String = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Some(match n.as_str() {
            "home" => Button::Home,
            "lock" | "power" | "sleep" | "side" => Button::Lock,
            "volumeup" | "volup" => Button::VolumeUp,
            "volumedown" | "voldown" => Button::VolumeDown,
            "siri" | "assistant" => Button::Siri,
            "appswitcher" | "recents" | "multitask" => Button::AppSwitcher,
            _ => return None,
        })
    }
}

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on Apple, Meta/Windows elsewhere.
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };

    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }

    fn union(self, o: Modifiers) -> Modifiers {
        Modifiers {
            shift: self.shift || o.shift,
            ctrl: self.ctrl || o.ctrl,
            alt: self.alt || o.alt,
            meta: self.meta || o.meta,
        }
    }
}

/// A keyboard key, optionally with modifiers via [`Key::Combo`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Enter,
    Backspace,
    Delete,
    Tab,
    Escape,
    Space,
    Up,
    Down,
    Left,
    Right,
    Char(char),
    Combo { modifiers: Modifiers, key: Box<Key> },
}

impl Key {
    /// Wrap a key with modifiers (no-op for empty modifiers).
    pub fn with(self, modifiers: Modifiers) -> Key {
        if modifiers.is_empty() {
            return self;
        }
        match self {
            Key::Combo { modifiers: m, key } => Key::Combo {
                modifiers: m.union(modifiers),
                key,
            },
            k => Key::Combo {
                modifiers,
                key: Box::new(k),
            },
        }
    }

    /// Split into the full modifier set and the base key.
    pub fn parts(&self) -> (Modifiers, &Key) {
        match self {
            Key::Combo { modifiers, key } => {
                let (inner, base) = key.parts();
                (modifiers.union(inner), base)
            }
            k => (Modifiers::NONE, k),
        }
    }

    /// Parse `enter`, `a`, `cmd+space`, `ctrl+shift+left`. A lone `+` is the plus key.
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        if s == "+" {
            return Some(Key::Char('+'));
        }
        let (rest, last) = match s.strip_suffix("++") {
            Some(r) => (r, "+"),
            None => match s.rsplit_once('+') {
                Some((r, l)) => (r, l),
                None => ("", s),
            },
        };
        let mut mods = Modifiers::NONE;
        for m in rest.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "shift" => mods.shift = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "cmd" | "command" | "meta" | "win" | "super" => mods.meta = true,
                _ => return None,
            }
        }
        let last = last.trim();
        let base = match last.to_ascii_lowercase().as_str() {
            "enter" | "return" => Key::Enter,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "tab" => Key::Tab,
            "escape" | "esc" => Key::Escape,
            "space" => Key::Space,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            _ => {
                let mut chars = last.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                Key::Char(c)
            }
        };
        Some(base.with(mods))
    }
}

/// Direction of a swipe or scroll: the direction the finger moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SwipeDirection {
    /// Unit vector (dx, dy) in screen coordinates (y grows downward).
    pub fn vector(self) -> (f32, f32) {
        match self {
            SwipeDirection::Up => (0.0, -1.0),
            SwipeDirection::Down => (0.0, 1.0),
            SwipeDirection::Left => (-1.0, 0.0),
            SwipeDirection::Right => (1.0, 0.0),
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            SwipeDirection::Up => SwipeDirection::Down,
            SwipeDirection::Down => SwipeDirection::Up,
            SwipeDirection::Left => SwipeDirection::Right,
            SwipeDirection::Right => SwipeDirection::Left,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => return None,
        })
    }
}

/// A complete input action as data (for logging, MCP and agent plans).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Gesture {
    Tap {
        at: Point,
    },
    DoubleTap {
        at: Point,
    },
    LongPress {
        at: Point,
        duration: Duration,
    },
    Swipe {
        from: Point,
        to: Point,
        duration: Duration,
    },
    Scroll {
        at: Point,
        direction: SwipeDirection,
        amount: f32,
    },
    TypeText {
        text: String,
    },
    Key {
        key: Key,
    },
    Button {
        button: Button,
    },
    LaunchApp {
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keys() {
        assert_eq!(Key::parse("Enter"), Some(Key::Enter));
        assert_eq!(Key::parse("a"), Some(Key::Char('a')));
        assert_eq!(Key::parse("+"), Some(Key::Char('+')));
        let k = Key::parse("cmd+shift+left").unwrap();
        let (m, base) = k.parts();
        assert!(m.meta && m.shift && !m.ctrl);
        assert_eq!(base, &Key::Left);
        let k2 = Key::parse("ctrl++").unwrap();
        let (m, base) = k2.parts();
        assert!(m.ctrl);
        assert_eq!(base, &Key::Char('+'));
        assert_eq!(Key::parse("hyper+x"), None);
        assert_eq!(Key::parse("abc"), None);
        assert_eq!(Key::Space.with(Modifiers::NONE), Key::Space);
    }

    #[test]
    fn parse_buttons_and_dirs() {
        assert_eq!(Button::parse("volume-up"), Some(Button::VolumeUp));
        assert_eq!(Button::parse("Power"), Some(Button::Lock));
        assert_eq!(Button::parse("x"), None);
        assert_eq!(SwipeDirection::parse("UP"), Some(SwipeDirection::Up));
        assert_eq!(SwipeDirection::Left.opposite(), SwipeDirection::Right);
        assert_eq!(SwipeDirection::Down.vector(), (0.0, 1.0));
    }
}
