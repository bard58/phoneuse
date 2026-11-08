//! The agent's tool set (mirrors upstream phoneuse MCP tools) and input parsing.
//!
//! **Coordinates:** every `x`/`y` is a fraction of the screen from 0.0 to 1.0,
//! measured from the top-left corner of one frame: (0.5, 0.5) is the centre,
//! (1.0, 1.0) the bottom-right corner. This matches `ap_core::Point`.
//!
//! The list and its order are fixed, so the tools prefix caches.

use std::time::Duration;

use ap_core::{Button, Key, Point, SwipeDirection};
use ap_llm::ToolDef;
use serde_json::{Value, json};

/// A parsed tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    Look,
    Wait {
        seconds: f32,
    },
    Tap {
        at: Point,
        irreversible: bool,
    },
    DoubleTap {
        at: Point,
        irreversible: bool,
    },
    LongPress {
        at: Point,
        duration: Duration,
        irreversible: bool,
    },
    Swipe {
        from: Point,
        to: Point,
        duration: Duration,
        irreversible: bool,
    },
    /// `direction` is the way the *content* moves into view ("down" shows what is below).
    Scroll {
        at: Point,
        direction: SwipeDirection,
        amount: f32,
    },
    TypeText {
        text: String,
        irreversible: bool,
    },
    PressKey {
        key: Key,
        irreversible: bool,
    },
    Button {
        button: Button,
    },
    OpenApp {
        app: String,
    },
    Home,
    Back,
    Confirm {
        action: String,
        details: Option<String>,
    },
    AskUser {
        question: String,
    },
    Unlock,
    Done {
        summary: String,
        success: bool,
    },
}

impl ToolCall {
    /// True for tools that change the phone (these consume an approval).
    pub fn is_action(&self) -> bool {
        matches!(
            self,
            ToolCall::Tap { .. }
                | ToolCall::DoubleTap { .. }
                | ToolCall::LongPress { .. }
                | ToolCall::Swipe { .. }
                | ToolCall::Scroll { .. }
                | ToolCall::TypeText { .. }
                | ToolCall::PressKey { .. }
                | ToolCall::Button { .. }
                | ToolCall::OpenApp { .. }
                | ToolCall::Home
                | ToolCall::Back
        )
    }

    /// The model marked this action as sending/paying/deleting/...
    pub fn irreversible(&self) -> bool {
        match self {
            ToolCall::Tap { irreversible, .. }
            | ToolCall::DoubleTap { irreversible, .. }
            | ToolCall::LongPress { irreversible, .. }
            | ToolCall::Swipe { irreversible, .. }
            | ToolCall::TypeText { irreversible, .. }
            | ToolCall::PressKey { irreversible, .. } => *irreversible,
            _ => false,
        }
    }
}

const XY: &str = "Fraction of the screen width (x) or height (y), 0.0 to 1.0 from the top-left corner of one frame; centre = 0.5.";
const IRREVERSIBLE: &str = "Set true when this action itself sends, posts, pays, buys, deletes, or changes an account, privacy or security setting. Needs an approved `confirm` call just before.";

fn coord() -> Value {
    json!({"type": "number", "minimum": 0.0, "maximum": 1.0, "description": XY})
}

fn irreversible() -> Value {
    json!({"type": "boolean", "description": IRREVERSIBLE})
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }),
    }
}

/// All tool definitions, in a fixed order.
pub fn definitions() -> Vec<ToolDef> {
    vec![
        tool(
            "look",
            "Take a fresh screenshot after the screen settles. Use it when you need to see the screen without acting; every action already returns the screen.",
            json!({}),
            &[],
        ),
        tool(
            "wait",
            "Wait, then return the screen again: for a page, spinner or result still loading. Choose 0.5 to 10 seconds from what you saw.",
            json!({"seconds": {"type": "number", "minimum": 0.0, "maximum": 10.0}}),
            &["seconds"],
        ),
        tool(
            "tap",
            "Tap once at (x, y). Coordinates are fractions of the screen (0.0-1.0). Aim at the centre of the element.",
            json!({"x": coord(), "y": coord(), "irreversible": irreversible()}),
            &["x", "y"],
        ),
        tool(
            "double_tap",
            "Tap twice quickly at (x, y) (zoom a photo or map, select a word). Fractions 0.0-1.0.",
            json!({"x": coord(), "y": coord(), "irreversible": irreversible()}),
            &["x", "y"],
        ),
        tool(
            "long_press",
            "Press and hold at (x, y) for `seconds` (default 0.8): context menus, rearranging. Fractions 0.0-1.0.",
            json!({"x": coord(), "y": coord(),
                   "seconds": {"type": "number", "minimum": 0.1, "maximum": 5.0},
                   "irreversible": irreversible()}),
            &["x", "y"],
        ),
        tool(
            "swipe",
            "Drag a finger from (x0, y0) to (x1, y1) over `seconds` (default 0.3): sliders, pull-down panels, custom gestures. Fractions 0.0-1.0. For lists and pages prefer `scroll`.",
            json!({"x0": coord(), "y0": coord(), "x1": coord(), "y1": coord(),
                   "seconds": {"type": "number", "minimum": 0.05, "maximum": 5.0},
                   "irreversible": irreversible()}),
            &["x0", "y0", "x1", "y1"],
        ),
        tool(
            "scroll",
            "Scroll content. `direction` is where you want to go: \"down\" shows what is below, \"up\" what is above. `amount` is the share of the screen to move (default 0.5). Optional x, y (fractions) pick the scrollable area; default is the centre.",
            json!({"direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                   "amount": {"type": "number", "minimum": 0.1, "maximum": 1.0},
                   "x": coord(), "y": coord()}),
            &["direction"],
        ),
        tool(
            "type_text",
            "Type text into the focused field (tap the field first). \"\\n\" presses Enter. The phone may autocorrect: check the result before sending. Never type a passcode, PIN or password.",
            json!({"text": {"type": "string"}, "irreversible": irreversible()}),
            &["text"],
        ),
        tool(
            "press_key",
            "Press a key or shortcut on the hardware keyboard: enter, backspace, delete, tab, escape, space, up, down, left, right, or combos like cmd+a, ctrl+shift+left.",
            json!({"key": {"type": "string"}, "irreversible": irreversible()}),
            &["key"],
        ),
        tool(
            "button",
            "Press a hardware or system button.",
            json!({"name": {"type": "string",
                    "enum": ["home", "lock", "volume_up", "volume_down", "siri", "app_switcher", "back"]}}),
            &["name"],
        ),
        tool(
            "open_app",
            "Open an app by name (\"Settings\") or by iOS bundle id / Android package name (\"com.apple.Preferences\", \"com.android.settings\"). Apps reopen where they were left.",
            json!({"app": {"type": "string"}}),
            &["app"],
        ),
        tool("home", "Go to the home screen.", json!({}), &[]),
        tool(
            "back",
            "Go back one screen (iPhone: swipe in from the left edge; Android: system back).",
            json!({}),
            &[],
        ),
        tool(
            "confirm",
            "Ask the owner to approve one irreversible action before you do it (send, post, pay, buy, delete, change account/security settings). Describe exactly what will happen. If approved, do that action next with irreversible=true.",
            json!({"action": {"type": "string", "description": "One line, e.g. \"Send the message 'On my way' to Alex in Messages\"."},
                   "details": {"type": "string"}}),
            &["action"],
        ),
        tool(
            "ask_user",
            "Ask the owner a question and wait for the answer: missing information, a choice only they can make, or when something unexpected happens.",
            json!({"question": {"type": "string"}}),
            &["question"],
        ),
        tool(
            "unlock",
            "On the lock screen: ask the owner to approve an unlock with their saved passcode. You never see the passcode. If it fails, do not retry; ask the owner to unlock.",
            json!({}),
            &[],
        ),
        tool(
            "done",
            "Finish the task. Give a short summary of what you did and found. success=false if you could not finish, with the reason.",
            json!({"summary": {"type": "string"}, "success": {"type": "boolean"}}),
            &["summary"],
        ),
    ]
}

fn num(input: &Value, k: &str) -> Result<Option<f32>, String> {
    match input.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| format!("`{k}` must be a number")),
    }
}

fn frac(input: &Value, k: &str) -> Result<f32, String> {
    let v = num(input, k)?.ok_or_else(|| format!("missing `{k}`"))?;
    if !(0.0..=1.0).contains(&v) {
        return Err(format!(
            "`{k}` = {v} is outside 0.0-1.0: coordinates are fractions of the screen"
        ));
    }
    Ok(v)
}

fn point(input: &Value, x: &str, y: &str) -> Result<Point, String> {
    Ok(Point::new(frac(input, x)?, frac(input, y)?))
}

fn string(input: &Value, k: &str) -> Result<String, String> {
    input
        .get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string `{k}`"))
}

fn flag(input: &Value, k: &str) -> bool {
    input.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn secs(input: &Value, k: &str, default: f32, min: f32, max: f32) -> Result<Duration, String> {
    let s = num(input, k)?.unwrap_or(default);
    if !s.is_finite() {
        return Err(format!("`{k}` must be finite"));
    }
    Ok(Duration::from_secs_f32(s.clamp(min, max)))
}

/// Parse one tool call. The error text goes back to the model.
pub fn parse(name: &str, input: &Value) -> Result<ToolCall, String> {
    if input.get("_invalid_json").is_some() {
        return Err("the tool arguments were not valid JSON".into());
    }
    let irr = flag(input, "irreversible");
    Ok(match name {
        "look" => ToolCall::Look,
        "wait" => ToolCall::Wait {
            seconds: num(input, "seconds")?.unwrap_or(1.0).clamp(0.0, 10.0),
        },
        "tap" => ToolCall::Tap {
            at: point(input, "x", "y")?,
            irreversible: irr,
        },
        "double_tap" => ToolCall::DoubleTap {
            at: point(input, "x", "y")?,
            irreversible: irr,
        },
        "long_press" => ToolCall::LongPress {
            at: point(input, "x", "y")?,
            duration: secs(input, "seconds", 0.8, 0.1, 5.0)?,
            irreversible: irr,
        },
        "swipe" => ToolCall::Swipe {
            from: point(input, "x0", "y0")?,
            to: point(input, "x1", "y1")?,
            duration: secs(input, "seconds", 0.3, 0.05, 5.0)?,
            irreversible: irr,
        },
        "scroll" => {
            let d = string(input, "direction")?;
            let direction = SwipeDirection::parse(&d)
                .ok_or_else(|| format!("unknown direction `{d}` (up, down, left, right)"))?;
            let at = match (input.get("x"), input.get("y")) {
                (None, None) => Point::CENTER,
                _ => point(input, "x", "y")?,
            };
            ToolCall::Scroll {
                at,
                direction,
                amount: num(input, "amount")?.unwrap_or(0.5).clamp(0.1, 1.0),
            }
        }
        "type_text" => {
            let text = string(input, "text")?;
            if text.is_empty() {
                return Err("`text` is empty".into());
            }
            ToolCall::TypeText {
                text,
                irreversible: irr,
            }
        }
        "press_key" => {
            let k = string(input, "key")?;
            ToolCall::PressKey {
                key: Key::parse(&k).ok_or_else(|| format!("unknown key `{k}`"))?,
                irreversible: irr,
            }
        }
        "button" => {
            let n = string(input, "name")?;
            ToolCall::Button {
                button: Button::parse(&n).ok_or_else(|| format!("unknown button `{n}`"))?,
            }
        }
        "open_app" => ToolCall::OpenApp {
            app: string(input, "app")?.trim().to_string(),
        },
        "home" => ToolCall::Home,
        "back" => ToolCall::Back,
        "confirm" => ToolCall::Confirm {
            action: string(input, "action")?,
            details: input
                .get("details")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        },
        "ask_user" => ToolCall::AskUser {
            question: string(input, "question")?,
        },
        "unlock" => ToolCall::Unlock,
        "done" => ToolCall::Done {
            summary: string(input, "summary")?,
            success: input
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        },
        other => return Err(format!("unknown tool `{other}`")),
    })
}

/// Text that looks like a passcode or PIN: 4 to 8 digits and nothing else.
pub fn looks_like_passcode(text: &str) -> bool {
    let t = text.trim_end_matches('\n');
    (4..=8).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit())
}

/// Common app names to bundle ids / package names.
pub fn app_alias(name: &str, ios: bool) -> Option<&'static str> {
    let n: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let ios_map: &[(&str, &str)] = &[
        ("settings", "com.apple.Preferences"),
        ("safari", "com.apple.mobilesafari"),
        ("messages", "com.apple.MobileSMS"),
        ("phone", "com.apple.mobilephone"),
        ("mail", "com.apple.mobilemail"),
        ("photos", "com.apple.mobileslideshow"),
        ("camera", "com.apple.camera"),
        ("maps", "com.apple.Maps"),
        ("notes", "com.apple.mobilenotes"),
        ("calendar", "com.apple.mobilecal"),
        ("clock", "com.apple.mobiletimer"),
        ("reminders", "com.apple.reminders"),
        ("appstore", "com.apple.AppStore"),
        ("music", "com.apple.Music"),
        ("files", "com.apple.DocumentsApp"),
        ("weather", "com.apple.weather"),
        ("contacts", "com.apple.MobileAddressBook"),
        ("calculator", "com.apple.calculator"),
        ("health", "com.apple.Health"),
        ("wallet", "com.apple.Passbook"),
    ];
    let android_map: &[(&str, &str)] = &[
        ("settings", "com.android.settings"),
        ("chrome", "com.android.chrome"),
        ("playstore", "com.android.vending"),
        ("gmail", "com.google.android.gm"),
        ("maps", "com.google.android.apps.maps"),
        ("youtube", "com.google.android.youtube"),
        ("photos", "com.google.android.apps.photos"),
        ("messages", "com.google.android.apps.messaging"),
        ("phone", "com.google.android.dialer"),
        ("contacts", "com.google.android.contacts"),
        ("calendar", "com.google.android.calendar"),
        ("clock", "com.google.android.deskclock"),
        ("camera", "com.android.camera2"),
        ("calculator", "com.google.android.calculator"),
        ("files", "com.google.android.documentsui"),
    ];
    let map = if ios { ios_map } else { android_map };
    map.iter().find(|(k, _)| *k == n).map(|(_, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_stable_and_valid() {
        let d = definitions();
        let names: Vec<&str> = d.iter().map(|t| t.name.as_str()).collect();
        for n in [
            "look",
            "wait",
            "tap",
            "long_press",
            "swipe",
            "scroll",
            "type_text",
            "press_key",
            "button",
            "open_app",
            "home",
            "back",
            "confirm",
            "ask_user",
            "unlock",
            "done",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        assert_eq!(definitions(), d);
        for t in &d {
            assert_eq!(t.input_schema["type"], "object");
        }
    }

    #[test]
    fn parsing() {
        assert_eq!(
            parse("tap", &json!({"x": 0.5, "y": 0.25})).unwrap(),
            ToolCall::Tap {
                at: Point::new(0.5, 0.25),
                irreversible: false
            }
        );
        assert!(
            parse("tap", &json!({"x": 195, "y": 400}))
                .unwrap_err()
                .contains("fractions")
        );
        assert!(parse("tap", &json!({"x": 0.5})).is_err());
        assert_eq!(
            parse("scroll", &json!({"direction": "down"})).unwrap(),
            ToolCall::Scroll {
                at: Point::CENTER,
                direction: SwipeDirection::Down,
                amount: 0.5
            }
        );
        assert!(matches!(
            parse("press_key", &json!({"key": "cmd+a"})).unwrap(),
            ToolCall::PressKey { .. }
        ));
        assert!(parse("press_key", &json!({"key": "nope"})).is_err());
        assert!(parse("button", &json!({"name": "app_switcher"})).is_ok());
        assert!(parse("frobnicate", &json!({})).is_err());
        assert!(
            parse("done", &json!({"summary": "ok"}))
                .map(|c| c
                    == ToolCall::Done {
                        summary: "ok".into(),
                        success: true
                    })
                .unwrap()
        );
    }

    #[test]
    fn passcode_and_alias() {
        assert!(looks_like_passcode("123456"));
        assert!(looks_like_passcode("1234\n"));
        assert!(!looks_like_passcode("12"));
        assert!(!looks_like_passcode("hello 1234"));
        assert_eq!(app_alias("App Store", true), Some("com.apple.AppStore"));
        assert_eq!(app_alias("Settings", false), Some("com.android.settings"));
        assert_eq!(app_alias("Nope", true), None);
    }
}
