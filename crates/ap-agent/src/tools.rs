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
    /// Ask the owner to approve `next`. An approval covers only that one call.
    Confirm {
        action: String,
        details: Option<String>,
        next: Box<ToolCall>,
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

/// Tools that change the phone. `confirm` names one of them as the next call.
pub const ACTION_TOOLS: &[&str] = &[
    "tap",
    "double_tap",
    "long_press",
    "swipe",
    "scroll",
    "type_text",
    "press_key",
    "button",
    "open_app",
    "home",
    "back",
];

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
            "Ask the owner to approve one irreversible action before you do it (send, post, pay, buy, delete, change account/security settings, press Enter or tap Send after typing, type a number the owner gave you). Describe exactly what will happen, and give the exact next call in `tool` and `input`. If approved, make exactly that call next, with irreversible=true. Any other call cancels the approval.",
            json!({"action": {"type": "string", "description": "One line, e.g. \"Send the message 'On my way' to Alex in Messages\"."},
                   "details": {"type": "string"},
                   "tool": {"type": "string", "enum": ACTION_TOOLS,
                            "description": "The tool you will call next if the owner approves."},
                   "input": {"type": "object",
                             "description": "The exact arguments of that next call, e.g. {\"x\": 0.9, \"y\": 0.93}."}}),
            &["action", "tool"],
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
        "confirm" => {
            let tool = string(input, "tool").map_err(|_| {
                "missing `tool`: name the exact call you will make next if the owner approves (for example \"tap\"), and give its arguments in `input`".to_string()
            })?;
            if !ACTION_TOOLS.contains(&tool.as_str()) {
                return Err(format!(
                    "`tool` = `{tool}` is not an action tool ({})",
                    ACTION_TOOLS.join(", ")
                ));
            }
            let args = match input.get("input") {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => return Err("`input` must be an object".into()),
            };
            let next = parse(&tool, &args).map_err(|e| format!("in `input`: {e}"))?;
            ToolCall::Confirm {
                action: string(input, "action")?,
                details: input
                    .get("details")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                next: Box::new(next),
            }
        }
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

/// Number of digits in `text` if it is digits only, after removing
/// whitespace (including `\r` and `\n`), `-`, `.` and `_`. Any Unicode digit
/// counts (full-width, Arabic-Indic, ...). `None` if anything else is left.
pub fn digit_only(text: &str) -> Option<u32> {
    let mut n = 0u32;
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '-' | '.' | '_') {
            continue;
        }
        if !c.is_numeric() {
            return None;
        }
        n += 1;
    }
    (n > 0).then_some(n)
}

/// Text that looks like a passcode or PIN: 4 or more digits and nothing else,
/// after the normalisation of [`digit_only`] (so "4821 ", "48 21", "4-8-2-1"
/// and full-width digits all count).
pub fn looks_like_passcode(text: &str) -> bool {
    digit_only(text).is_some_and(|n| n >= 4)
}

/// Digits this call enters, if it enters digits only: digit-only `type_text`,
/// or `press_key` with a digit and no modifier. Used to catch a passcode
/// typed in pieces.
pub fn digit_entry(call: &ToolCall) -> Option<u32> {
    match call {
        ToolCall::TypeText { text, .. } => digit_only(text),
        ToolCall::PressKey { key, .. } => match key.parts() {
            (m, Key::Char(c)) if m.is_empty() && c.is_numeric() => Some(1),
            _ => None,
        },
        _ => None,
    }
}

/// True when `a` and `b` are the same action: same tool and arguments, points
/// within 0.03 of the screen. The `irreversible` flag is ignored.
pub fn same_action(a: &ToolCall, b: &ToolCall) -> bool {
    use ToolCall as T;
    const TOL: f32 = 0.03;
    let near = |p: &Point, q: &Point| (p.x - q.x).abs() <= TOL && (p.y - q.y).abs() <= TOL;
    let secs = |x: &Duration, y: &Duration| (x.as_secs_f32() - y.as_secs_f32()).abs() <= 0.5;
    match (a, b) {
        (T::Tap { at: p, .. }, T::Tap { at: q, .. })
        | (T::DoubleTap { at: p, .. }, T::DoubleTap { at: q, .. }) => near(p, q),
        (
            T::LongPress {
                at: p, duration: d, ..
            },
            T::LongPress {
                at: q, duration: e, ..
            },
        ) => near(p, q) && secs(d, e),
        (
            T::Swipe {
                from: p0, to: p1, ..
            },
            T::Swipe {
                from: q0, to: q1, ..
            },
        ) => near(p0, q0) && near(p1, q1),
        (
            T::Scroll {
                at: p,
                direction: d,
                ..
            },
            T::Scroll {
                at: q,
                direction: e,
                ..
            },
        ) => near(p, q) && d == e,
        (T::TypeText { text: x, .. }, T::TypeText { text: y, .. }) => x == y,
        (T::PressKey { key: x, .. }, T::PressKey { key: y, .. }) => x == y,
        (T::Button { button: x }, T::Button { button: y }) => x == y,
        (T::OpenApp { app: x }, T::OpenApp { app: y }) => x.eq_ignore_ascii_case(y),
        (T::Home, T::Home) | (T::Back, T::Back) => true,
        _ => false,
    }
}

/// One line for the owner: the exact call that an approval allows.
pub fn describe(call: &ToolCall) -> String {
    match call {
        ToolCall::Tap { at, .. } => format!("tap at ({:.3}, {:.3})", at.x, at.y),
        ToolCall::DoubleTap { at, .. } => format!("double tap at ({:.3}, {:.3})", at.x, at.y),
        ToolCall::LongPress { at, .. } => format!("long press at ({:.3}, {:.3})", at.x, at.y),
        ToolCall::Swipe { from, to, .. } => format!(
            "swipe from ({:.3}, {:.3}) to ({:.3}, {:.3})",
            from.x, from.y, to.x, to.y
        ),
        ToolCall::Scroll { direction, .. } => format!("scroll {direction:?}"),
        ToolCall::TypeText { text, .. } => format!("type {text:?}"),
        ToolCall::PressKey { key, .. } => format!("press key {key:?}"),
        ToolCall::Button { button } => format!("press button {button:?}"),
        ToolCall::OpenApp { app } => format!("open app {app:?}"),
        ToolCall::Home => "go to the home screen".into(),
        ToolCall::Back => "go back".into(),
        other => format!("{other:?}"),
    }
}

/// Replace runs of 4 or more digits with a placeholder.
fn redact_digit_runs(text: &str) -> String {
    fn flush(run: &mut String, out: &mut String) {
        let n = run.chars().count();
        if n >= 4 {
            out.push_str(&format!("[{n} digits]"));
        } else {
            out.push_str(run);
        }
        run.clear();
    }
    let mut out = String::new();
    let mut run = String::new();
    for c in text.chars() {
        if c.is_numeric() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn redact_text(text: &str) -> String {
    match digit_only(text) {
        Some(n) => format!("[{n} digits, redacted]"),
        None => redact_digit_runs(text),
    }
}

/// Tool input as it may appear in events and logs: digit-only text (a
/// possible passcode or code), runs of 4+ digits and digit keys are hidden.
/// The model still gets the real call.
pub fn redact_input(name: &str, input: &Value) -> Value {
    let mut v = input.clone();
    match name {
        "type_text" => {
            if let Some(t) = v.get("text").and_then(Value::as_str).map(redact_text) {
                v["text"] = Value::String(t);
            }
        }
        "press_key" => {
            let digit = v
                .get("key")
                .and_then(Value::as_str)
                .and_then(Key::parse)
                .is_some_and(|k| matches!(k.parts().1, Key::Char(c) if c.is_numeric()));
            if digit {
                v["key"] = Value::String("[digit, redacted]".into());
            }
        }
        "confirm" => {
            if let (Some(tool), Some(inner)) = (
                v.get("tool").and_then(Value::as_str).map(str::to_string),
                v.get("input").cloned(),
            ) {
                v["input"] = redact_input(&tool, &inner);
            }
            for k in ["action", "details"] {
                if let Some(t) = v.get(k).and_then(Value::as_str).map(redact_digit_runs) {
                    v[k] = Value::String(t);
                }
            }
        }
        _ => {}
    }
    v
}

/// App ids where a tap right after typing may send a message or a payment.
pub fn sensitive_app(id: &str) -> bool {
    const PARTS: &[&str] = &[
        "sms",
        "mms",
        "messag",
        "whatsapp",
        "telegram",
        "signal",
        "messenger",
        "orca",
        "slack",
        "discord",
        "mail",
        "outlook",
        "teams",
        "skype",
        "wechat",
        "viber",
        "instagram",
        "twitter",
        "snapchat",
        "threads",
        "linkedin",
        "facebook",
        "reddit",
        "dialer",
        "mobilephone",
        "pay",
        "wallet",
        "passbook",
        "bank",
        "venmo",
        "cash",
        "revolut",
        "transferwise",
        "zelle",
        "coinbase",
        "binance",
        "crypto",
        "trading",
        "robinhood",
        "amazon",
        "ebay",
        "shop",
        "store",
        "vending",
        "uber",
        "doordash",
        "booking",
        "airbnb",
    ];
    const WHOLE: &[&str] = &["gm", "mm", "line", "x", "wise", "chat", "talk"];
    id.to_lowercase()
        .split('.')
        .any(|seg| WHOLE.contains(&seg) || PARTS.iter().any(|p| seg.contains(p)))
}

/// Screen text (for example OCR lines in the observation) that suggests a lock
/// screen or a passcode / password field.
pub fn lock_screen_hint(text: &str) -> bool {
    const HINTS: &[&str] = &[
        "passcode",
        "enter pin",
        "enter your pin",
        "wrong pin",
        "enter password",
        "enter your password",
        "touch id",
        "face id",
        "swipe up to unlock",
        "slide to unlock",
        "press home to unlock",
        "draw your pattern",
        "draw pattern",
        "emergency call",
    ];
    let t = text.to_lowercase();
    HINTS.iter().any(|h| t.contains(h))
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
        assert!(looks_like_passcode("4821 "));
        assert!(looks_like_passcode(" 48 21\r\n"));
        assert!(looks_like_passcode("4-8-2-1"));
        assert!(looks_like_passcode("\u{ff14}\u{ff18}\u{ff12}\u{ff11}"));
        assert!(looks_like_passcode("123456789012"));
        assert!(!looks_like_passcode("12"));
        assert!(!looks_like_passcode("hello 1234"));
        assert_eq!(digit_only("48"), Some(2));
        assert_eq!(
            digit_entry(&parse("press_key", &json!({"key": "7"})).unwrap()),
            Some(1)
        );
        assert_eq!(
            digit_entry(&parse("press_key", &json!({"key": "shift+7"})).unwrap()),
            None
        );
        assert_eq!(app_alias("App Store", true), Some("com.apple.AppStore"));
        assert_eq!(app_alias("Settings", false), Some("com.android.settings"));
        assert_eq!(app_alias("Nope", true), None);
    }

    #[test]
    fn confirm_names_the_next_call() {
        let c = parse(
            "confirm",
            &json!({"action": "Send", "tool": "tap", "input": {"x": 0.9, "y": 0.9}}),
        )
        .unwrap();
        let ToolCall::Confirm { next, .. } = c else {
            panic!("not confirm")
        };
        assert!(same_action(
            &next,
            &parse("tap", &json!({"x": 0.91, "y": 0.89, "irreversible": true})).unwrap()
        ));
        assert!(!same_action(
            &next,
            &parse("tap", &json!({"x": 0.5, "y": 0.9})).unwrap()
        ));
        assert!(parse("confirm", &json!({"action": "Send"})).is_err());
        assert!(parse("confirm", &json!({"action": "x", "tool": "done"})).is_err());
        assert!(parse("confirm", &json!({"action": "x", "tool": "tap"})).is_err());
        assert!(matches!(
            parse("confirm", &json!({"action": "x", "tool": "home"})).unwrap(),
            ToolCall::Confirm { .. }
        ));
    }

    #[test]
    fn redaction_and_context() {
        assert_eq!(
            redact_input("type_text", &json!({"text": "48 21"}))["text"],
            "[4 digits, redacted]"
        );
        assert_eq!(
            redact_input("type_text", &json!({"text": "code 482193 ok"}))["text"],
            "code [6 digits] ok"
        );
        assert_eq!(
            redact_input("type_text", &json!({"text": "hi 12"}))["text"],
            "hi 12"
        );
        assert_eq!(
            redact_input("press_key", &json!({"key": "5"}))["key"],
            "[digit, redacted]"
        );
        assert_eq!(
            redact_input(
                "confirm",
                &json!({"action": "type 4821", "tool": "type_text", "input": {"text": "4821"}})
            )["input"]["text"],
            "[4 digits, redacted]"
        );
        assert!(sensitive_app("com.apple.MobileSMS"));
        assert!(sensitive_app("com.whatsapp"));
        assert!(sensitive_app("com.google.android.gm"));
        assert!(sensitive_app("com.apple.Passbook"));
        assert!(!sensitive_app("com.apple.Preferences"));
        assert!(!sensitive_app("com.apple.mobilenotes"));
        assert!(lock_screen_hint("Enter Passcode\nEmergency"));
        assert!(!lock_screen_hint("Current screen (settled)."));
    }
}
