//! Pure helpers: parse adb / shell output, build shell commands, escape text,
//! map keys to Android keycodes. Everything here is unit-tested without a device.

use ap_core::{Button, Error, Key, Modifiers, Point, Result, Size, SwipeDirection};

// ---------------------------------------------------------------- devices -l

/// Connection state reported by `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// Ready to use.
    Device,
    /// The phone has not accepted this PC's RSA key yet.
    Unauthorized,
    Offline,
    Authorizing,
    /// USB permission problem on the PC side.
    NoPermissions,
    /// recovery, sideload, bootloader, host, ...
    Other(String),
}

/// One line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    pub serial: String,
    pub state: DeviceState,
    pub model: Option<String>,
    pub product: Option<String>,
    pub device: Option<String>,
    pub transport_id: Option<String>,
}

impl DeviceEntry {
    /// True for Wi-Fi devices: `ip:port` serials and mDNS `adb-...._adb-tls-connect._tcp`.
    pub fn is_wireless(&self) -> bool {
        crate::adb::is_host_port(&self.serial) || self.serial.contains("._adb-tls-connect._tcp")
    }
}

/// Parse the reply of `host:devices-l`.
pub fn parse_devices(text: &str) -> Vec<DeviceEntry> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("List of devices") && !l.starts_with('*'))
        .filter_map(|line| {
            let mut toks = line.split_whitespace();
            let serial = toks.next()?.to_string();
            let mut state_tok = toks.next()?.to_string();
            let mut rest: Vec<&str> = toks.collect();
            if state_tok == "no" && rest.first() == Some(&"permissions") {
                state_tok = "no permissions".into();
                // Skip the "(user ... ); see [http://...]" explanation.
                rest.retain(|t| t.contains(':') && !t.contains("//") && !t.starts_with('('));
            }
            let state = match state_tok.as_str() {
                "device" => DeviceState::Device,
                "unauthorized" => DeviceState::Unauthorized,
                "offline" => DeviceState::Offline,
                "authorizing" => DeviceState::Authorizing,
                "no permissions" => DeviceState::NoPermissions,
                s => DeviceState::Other(s.to_string()),
            };
            let mut e = DeviceEntry {
                serial,
                state,
                model: None,
                product: None,
                device: None,
                transport_id: None,
            };
            for t in rest {
                if let Some((k, v)) = t.split_once(':') {
                    let v = Some(v.to_string());
                    match k {
                        "model" => e.model = v,
                        "product" => e.product = v,
                        "device" => e.device = v,
                        "transport_id" => e.transport_id = v,
                        _ => {}
                    }
                }
            }
            Some(e)
        })
        .collect()
}

// ---------------------------------------------------------------- wm size / density

/// Parse `wm size`. The override size wins, because input and screencap use it.
pub fn parse_wm_size(text: &str) -> Option<(u32, u32)> {
    let pick = |label: &str| {
        text.lines().find_map(|l| {
            let v = l.trim().strip_prefix(label)?.trim();
            let (w, h) = v.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
    };
    pick("Override size:").or_else(|| pick("Physical size:"))
}

/// Parse `wm density` (dpi). The override density wins.
pub fn parse_wm_density(text: &str) -> Option<u32> {
    let pick = |label: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(label)?.trim().parse().ok())
    };
    pick("Override density:").or_else(|| pick("Physical density:"))
}

/// Size in density-independent points (dp = px * 160 / dpi).
pub fn pixels_to_points(pixels: Size, dpi: u32) -> Size {
    if dpi == 0 {
        return pixels;
    }
    let f = 160.0 / dpi as f32;
    Size::new((pixels.width * f).round(), (pixels.height * f).round())
}

/// Parsed output of the info script built by [`INFO_SCRIPT`].
#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    pub manufacturer: String,
    pub model: String,
    pub release: String,
    pub sdk: u32,
    pub pixels: Option<(u32, u32)>,
    pub dpi: Option<u32>,
}

/// One round trip for model, version, size and density.
pub const INFO_SCRIPT: &str = "getprop ro.product.manufacturer; getprop ro.product.model; \
     getprop ro.build.version.release; getprop ro.build.version.sdk; echo @@AP@@; \
     wm size 2>&1; echo @@AP@@; wm density 2>&1";

/// Parse the output of [`INFO_SCRIPT`].
pub fn parse_info(text: &str) -> Props {
    let text = text.replace("\r\n", "\n");
    let mut parts = text.split("@@AP@@");
    let props: Vec<&str> = parts.next().unwrap_or("").lines().map(str::trim).collect();
    let get = |i: usize| props.get(i).copied().unwrap_or("").to_string();
    Props {
        manufacturer: get(0),
        model: get(1),
        release: get(2),
        sdk: get(3).parse().unwrap_or(0),
        pixels: parse_wm_size(parts.next().unwrap_or("")),
        dpi: parse_wm_density(parts.next().unwrap_or("")),
    }
}

/// "Google Pixel 8", without repeating the maker ("samsung SM-S911B").
pub fn display_name(manufacturer: &str, model: &str) -> String {
    match (manufacturer.trim(), model.trim()) {
        ("", "") => "Android device".into(),
        ("", m) => m.into(),
        (mf, "") => mf.into(),
        (mf, m) if m.to_lowercase().starts_with(&mf.to_lowercase()) => m.into(),
        (mf, m) => format!("{mf} {m}"),
    }
}

// ---------------------------------------------------------------- coordinates

/// Normalized point to device pixels (clamped inside the screen).
pub fn to_px(p: Point, pixels: Size) -> (u32, u32) {
    p.to_pixels(pixels)
}

/// Duration for `input swipe`, in ms (1..=60000).
pub fn duration_ms(d: std::time::Duration) -> u64 {
    (d.as_millis() as u64).clamp(1, 60_000)
}

/// Start and end of a scroll gesture: the finger moves `amount` of the
/// screen in `direction`, centred on `at`, clamped to the screen.
pub fn scroll_points(at: Point, direction: SwipeDirection, amount: f32) -> (Point, Point) {
    let amount = if amount.is_finite() {
        amount.clamp(0.05, 1.0)
    } else {
        0.5
    };
    let (dx, dy) = direction.vector();
    let half = amount / 2.0;
    let from = at.offset(-dx * half, -dy * half);
    let to = at.offset(dx * half, dy * half);
    (from, to)
}

// ---------------------------------------------------------------- shell quoting / text

/// Quote one argument for `/system/bin/sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// True if `input text` can type this text (printable ASCII, newline, tab).
pub fn is_input_text_safe(text: &str) -> bool {
    text.chars()
        .all(|c| (c.is_ascii() && !c.is_ascii_control()) || c == '\n' || c == '\t')
}

/// Escape one run of printable ASCII for `input text`: the shell layer gets
/// single quotes; the `input` layer turns `%s` into a space, so spaces become
/// `%s`. A literal `%s` cannot be escaped, so the run is split between `%` and
/// `s` into separate `input text` calls.
pub fn input_text_args(run: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = run;
    while let Some(i) = rest.find("%s") {
        pieces.push(&rest[..=i]);
        rest = &rest[i + 1..];
    }
    pieces.push(rest);
    pieces
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| shell_quote(&p.replace(' ', "%s")))
        .collect()
}

/// Maximum characters of text per `input text` call.
const TEXT_CHUNK: usize = 200;
/// Keep each `exec:` request well under old adbd's 4 KiB limit.
const SCRIPT_LIMIT: usize = 3000;

/// Shell scripts that type printable-ASCII `text` (newline = Enter, tab = Tab).
/// Returns an error for other characters; see [`is_input_text_safe`].
pub fn text_scripts(text: &str) -> Result<Vec<String>> {
    if !is_input_text_safe(text) {
        return Err(Error::InvalidArgument(
            "text has characters `input text` cannot type".into(),
        ));
    }
    let mut cmds = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, cmds: &mut Vec<String>| {
        let chars: Vec<char> = run.chars().collect();
        for chunk in chars.chunks(TEXT_CHUNK) {
            let s: String = chunk.iter().collect();
            for arg in input_text_args(&s) {
                cmds.push(format!("input text {arg}"));
            }
        }
        run.clear();
    };
    for c in text.chars() {
        match c {
            '\n' => {
                flush(&mut run, &mut cmds);
                cmds.push(format!("input keyevent {KEYCODE_ENTER}"));
            }
            '\t' => {
                flush(&mut run, &mut cmds);
                cmds.push(format!("input keyevent {KEYCODE_TAB}"));
            }
            c => run.push(c),
        }
    }
    flush(&mut run, &mut cmds);
    Ok(join_scripts(cmds))
}

/// Join commands with `&&` into as few scripts as fit [`SCRIPT_LIMIT`].
pub fn join_scripts(cmds: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in cmds {
        match out.last_mut() {
            Some(last) if last.len() + c.len() + 4 <= SCRIPT_LIMIT => {
                last.push_str(" && ");
                last.push_str(&c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// ADBKeyBoard broadcast for text `input text` cannot type (any Unicode).
pub fn adb_keyboard_script(text: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Base64 is shell-safe ([A-Za-z0-9+/=]).
    format!("am broadcast -a ADB_INPUT_B64 --es msg {b64}")
}

/// Package and IME id of ADBKeyBoard (github.com/senzhk/ADBKeyBoard).
pub const ADB_KEYBOARD_PKG: &str = "com.android.adbkeyboard";
pub const ADB_KEYBOARD_IME: &str = "com.android.adbkeyboard/.AdbIME";

// ---------------------------------------------------------------- keys / buttons

pub const KEYCODE_HOME: u32 = 3;
pub const KEYCODE_BACK: u32 = 4;
pub const KEYCODE_VOLUME_UP: u32 = 24;
pub const KEYCODE_VOLUME_DOWN: u32 = 25;
pub const KEYCODE_POWER: u32 = 26;
pub const KEYCODE_TAB: u32 = 61;
pub const KEYCODE_SPACE: u32 = 62;
pub const KEYCODE_ENTER: u32 = 66;
pub const KEYCODE_DEL: u32 = 67;
pub const KEYCODE_ESCAPE: u32 = 111;
pub const KEYCODE_FORWARD_DEL: u32 = 112;
pub const KEYCODE_APP_SWITCH: u32 = 187;
pub const KEYCODE_ASSIST: u32 = 219;
pub const KEYCODE_SHIFT_LEFT: u32 = 59;
pub const KEYCODE_ALT_LEFT: u32 = 57;
pub const KEYCODE_CTRL_LEFT: u32 = 113;
pub const KEYCODE_META_LEFT: u32 = 117;

/// First SDK with `input keycombination` (Android 13).
pub const KEYCOMBINATION_SDK: u32 = 33;

/// Android keycode for a hardware/system button.
pub fn button_keycode(b: Button) -> u32 {
    match b {
        Button::Home => KEYCODE_HOME,
        Button::Lock => KEYCODE_POWER,
        Button::VolumeUp => KEYCODE_VOLUME_UP,
        Button::VolumeDown => KEYCODE_VOLUME_DOWN,
        Button::AppSwitcher => KEYCODE_APP_SWITCH,
        Button::Siri => KEYCODE_ASSIST,
        Button::Back => KEYCODE_BACK,
    }
}

/// Keycode of a base key, plus whether Shift is needed on a US layout.
pub fn keycode(key: &Key) -> Option<(u32, bool)> {
    Some(match key {
        Key::Enter => (KEYCODE_ENTER, false),
        Key::Backspace => (KEYCODE_DEL, false),
        Key::Delete => (KEYCODE_FORWARD_DEL, false),
        Key::Tab => (KEYCODE_TAB, false),
        Key::Escape => (KEYCODE_ESCAPE, false),
        Key::Space => (KEYCODE_SPACE, false),
        Key::Up => (19, false),
        Key::Down => (20, false),
        Key::Left => (21, false),
        Key::Right => (22, false),
        Key::Char(c) => char_keycode(*c)?,
        Key::Combo { .. } => return None,
    })
}

fn char_keycode(c: char) -> Option<(u32, bool)> {
    Some(match c {
        'a'..='z' => (29 + (c as u32 - 'a' as u32), false),
        'A'..='Z' => (29 + (c as u32 - 'A' as u32), true),
        '0'..='9' => (7 + (c as u32 - '0' as u32), false),
        ' ' => (KEYCODE_SPACE, false),
        ',' => (55, false),
        '.' => (56, false),
        '`' => (68, false),
        '-' => (69, false),
        '=' => (70, false),
        '[' => (71, false),
        ']' => (72, false),
        '\\' => (73, false),
        ';' => (74, false),
        '\'' => (75, false),
        '/' => (76, false),
        '@' => (77, false),
        '+' => (81, false),
        '*' => (17, false),
        '#' => (18, false),
        '!' => (8, true),
        '$' => (11, true),
        '%' => (12, true),
        '^' => (13, true),
        '&' => (14, true),
        '(' => (16, true),
        ')' => (7, true),
        '_' => (69, true),
        '{' => (71, true),
        '}' => (72, true),
        '|' => (73, true),
        ':' => (74, true),
        '"' => (75, true),
        '<' => (55, true),
        '>' => (56, true),
        '?' => (76, true),
        '~' => (68, true),
        '\n' => (KEYCODE_ENTER, false),
        '\t' => (KEYCODE_TAB, false),
        _ => return None,
    })
}

/// What to do for one [`Key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Type this text (plain characters go through `input text`).
    Text(String),
    /// Run this shell command.
    Shell(String),
}

/// Plan a key press for a device with this SDK level.
pub fn key_action(key: &Key, sdk: u32) -> Result<KeyAction> {
    let (mods, base) = key.parts();
    if let Key::Char(c) = base {
        // A plain or shifted character: typing it is the most reliable path.
        if mods.is_empty() || (mods == shift_only() && !c.is_whitespace()) {
            let c = if mods.shift { shifted(*c) } else { *c };
            return Ok(KeyAction::Text(c.to_string()));
        }
    }
    let (code, needs_shift) = keycode(base)
        .ok_or_else(|| Error::unsupported(format!("key {base:?} has no Android keycode")))?;
    if mods.is_empty() && !needs_shift {
        return Ok(KeyAction::Shell(format!("input keyevent {code}")));
    }
    if sdk != 0 && sdk < KEYCOMBINATION_SDK {
        return Err(Error::unsupported(format!(
            "key combination {key:?}: needs Android 13+ (`input keycombination`), device has SDK {sdk}"
        )));
    }
    let mut codes = Vec::new();
    if mods.ctrl {
        codes.push(KEYCODE_CTRL_LEFT);
    }
    if mods.alt {
        codes.push(KEYCODE_ALT_LEFT);
    }
    if mods.shift || needs_shift {
        codes.push(KEYCODE_SHIFT_LEFT);
    }
    if mods.meta {
        codes.push(KEYCODE_META_LEFT);
    }
    codes.push(code);
    let list: Vec<String> = codes.iter().map(u32::to_string).collect();
    Ok(KeyAction::Shell(format!(
        "input keycombination {}",
        list.join(" ")
    )))
}

fn shift_only() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::NONE
    }
}

fn shifted(c: char) -> char {
    c.to_ascii_uppercase()
}

/// Detect an error printed by `input`, `am`, `pm` or `monkey`.
pub fn shell_error(out: &str) -> Option<String> {
    let markers = [
        "Error:",
        "Exception",
        "Usage: input",
        "Unknown command",
        "inaccessible or not found",
        "not found",
        "Permission denied",
    ];
    markers
        .iter()
        .any(|m| out.contains(m))
        .then(|| out.trim().lines().take(4).collect::<Vec<_>>().join(" | "))
}

// ---------------------------------------------------------------- apps / ui tree

/// Android package names: letters, digits, `_` and `.`.
pub fn is_package_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.split('.').all(|seg| {
            !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Foreground package from `dumpsys window` output. Reads `mCurrentFocus`
/// first, then `mFocusedApp`. A focus window without a package (status bar,
/// notification shade) gives `None`.
pub fn parse_foreground_package(text: &str) -> Option<String> {
    for key in ["mCurrentFocus=", "mFocusedApp="] {
        for line in text.lines() {
            let Some(rest) = line.split_once(key).map(|(_, r)| r) else {
                continue;
            };
            let found = rest
                .split(|c: char| c.is_whitespace() || c == '{' || c == '}')
                .filter_map(|tok| tok.split_once('/').map(|(pkg, _)| pkg))
                .find(|pkg| is_package_name(pkg) && pkg.contains('.'));
            if let Some(pkg) = found {
                return Some(pkg.to_string());
            }
        }
    }
    None
}

/// Parse `pm list packages` into sorted package names.
pub fn parse_packages(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Cut the XML out of `uiautomator dump` output (it appends a status line).
pub fn extract_ui_xml(text: &str) -> Option<String> {
    let start = text.find("<?xml").or_else(|| text.find("<hierarchy"))?;
    let end_tag = "</hierarchy>";
    let end = text.rfind(end_tag)? + end_tag.len();
    (end > start).then(|| text[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn devices_list() {
        let out = "List of devices attached\n\
            emulator-5554          device product:sdk_gphone64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
            R58N123ABC             unauthorized usb:1-1 transport_id:3\n\
            192.168.1.20:5555      offline transport_id:4\n\
            adb-XYZ._adb-tls-connect._tcp device product:husky model:Pixel_8_Pro device:husky transport_id:5\n\
            0123456789ABCDEF       no permissions (user in plugdev group; are your udev rules wrong?); see [http://developer.android.com/tools/device.html] usb:1-2 transport_id:6\n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 5);
        assert_eq!(d[0].serial, "emulator-5554");
        assert_eq!(d[0].state, DeviceState::Device);
        assert_eq!(d[0].model.as_deref(), Some("sdk_gphone64_x86_64"));
        assert_eq!(d[0].transport_id.as_deref(), Some("1"));
        assert_eq!(d[1].state, DeviceState::Unauthorized);
        assert_eq!(d[2].state, DeviceState::Offline);
        assert!(d[2].is_wireless());
        assert!(d[3].is_wireless());
        assert!(!d[0].is_wireless());
        assert_eq!(d[4].state, DeviceState::NoPermissions);
        assert_eq!(d[4].transport_id.as_deref(), Some("6"));
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn wm_parsing() {
        assert_eq!(
            parse_wm_size("Physical size: 1080x2400\n"),
            Some((1080, 2400))
        );
        assert_eq!(
            parse_wm_size("Physical size: 1440x3120\r\nOverride size: 1080x2340\r\n"),
            Some((1080, 2340))
        );
        assert_eq!(parse_wm_size("garbage"), None);
        assert_eq!(parse_wm_density("Physical density: 420\n"), Some(420));
        assert_eq!(
            parse_wm_density("Physical density: 560\nOverride density: 480\n"),
            Some(480)
        );
        assert_eq!(
            pixels_to_points(Size::new(1080.0, 2400.0), 420),
            Size::new(411.0, 914.0)
        );
        assert_eq!(
            pixels_to_points(Size::new(10.0, 20.0), 0),
            Size::new(10.0, 20.0)
        );
    }

    #[test]
    fn info_script_output() {
        let out = "Google\r\nPixel 8\r\n14\r\n34\r\n@@AP@@\r\nPhysical size: 1080x2400\r\n@@AP@@\r\nPhysical density: 420\r\n";
        let p = parse_info(out);
        assert_eq!(p.manufacturer, "Google");
        assert_eq!(p.model, "Pixel 8");
        assert_eq!(p.release, "14");
        assert_eq!(p.sdk, 34);
        assert_eq!(p.pixels, Some((1080, 2400)));
        assert_eq!(p.dpi, Some(420));
        assert_eq!(display_name("Google", "Pixel 8"), "Google Pixel 8");
        assert_eq!(
            display_name("samsung", "SAMSUNG-SM-G900A"),
            "SAMSUNG-SM-G900A"
        );
        assert_eq!(display_name("", ""), "Android device");
    }

    #[test]
    fn coordinates() {
        let px = Size::new(1080.0, 2400.0);
        assert_eq!(to_px(Point::CENTER, px), (540, 1200));
        assert_eq!(to_px(Point::new(0.0, 0.0), px), (0, 0));
        assert_eq!(to_px(Point::new(1.0, 1.0), px), (1079, 2399));
        assert_eq!(to_px(Point::new(0.25, 0.1), px), (270, 240));
        assert_eq!(duration_ms(Duration::from_millis(0)), 1);
        assert_eq!(duration_ms(Duration::from_secs(120)), 60_000);
        let (a, b) = scroll_points(Point::CENTER, SwipeDirection::Up, 0.4);
        assert!((a.y - 0.7).abs() < 1e-6 && (b.y - 0.3).abs() < 1e-6);
        assert_eq!(a.x, 0.5);
        let (a, b) = scroll_points(Point::new(0.9, 0.5), SwipeDirection::Right, 0.6);
        assert!((a.x - 0.6).abs() < 1e-6);
        assert_eq!(b.x, 1.0);
    }

    #[test]
    fn escaping() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(input_text_args("hello world"), vec!["'hello%sworld'"]);
        assert_eq!(
            input_text_args("a;b&&c|d$(rm -rf /)`x`\"q\"<>"),
            vec!["'a;b&&c|d$(rm%s-rf%s/)`x`\"q\"<>'"]
        );
        // Literal %s must not turn into a space.
        assert_eq!(input_text_args("100%sure"), vec!["'100%'", "'sure'"]);
        assert_eq!(input_text_args("%s%s"), vec!["'%'", "'s%'", "'s'"]);
        assert!(input_text_args("").is_empty());
        assert_eq!(input_text_args("50% off"), vec!["'50%%soff'"]);
    }

    #[test]
    fn text_scripts_split_lines() {
        let s = text_scripts("hi there\nok\tgo").unwrap();
        assert_eq!(
            s,
            vec![
                "input text 'hi%sthere' && input keyevent 66 && input text 'ok' && \
                 input keyevent 61 && input text 'go'"
            ]
        );
        assert!(text_scripts("").unwrap().is_empty());
        assert!(text_scripts("merhaba dünya").is_err());
        assert!(!is_input_text_safe("emoji 😀"));
        let long = "x".repeat(10_000);
        let scripts = text_scripts(&long).unwrap();
        assert!(scripts.len() > 1);
        assert!(scripts.iter().all(|s| s.len() <= SCRIPT_LIMIT));
        let typed: usize = scripts
            .iter()
            .flat_map(|s| s.split(" && "))
            .map(|c| c.len() - "input text ''".len())
            .sum();
        assert_eq!(typed, 10_000);
        let k = adb_keyboard_script("dünya");
        assert_eq!(k, "am broadcast -a ADB_INPUT_B64 --es msg ZMO8bnlh");
    }

    #[test]
    fn key_mapping() {
        assert_eq!(
            key_action(&Key::Enter, 30).unwrap(),
            KeyAction::Shell("input keyevent 66".into())
        );
        assert_eq!(
            key_action(&Key::Backspace, 30).unwrap(),
            KeyAction::Shell("input keyevent 67".into())
        );
        assert_eq!(
            key_action(&Key::Delete, 30).unwrap(),
            KeyAction::Shell("input keyevent 112".into())
        );
        assert_eq!(
            key_action(&Key::Char('a'), 30).unwrap(),
            KeyAction::Text("a".into())
        );
        assert_eq!(
            key_action(&Key::parse("shift+a").unwrap(), 30).unwrap(),
            KeyAction::Text("A".into())
        );
        assert_eq!(
            key_action(&Key::parse("ctrl+a").unwrap(), 34).unwrap(),
            KeyAction::Shell("input keycombination 113 29".into())
        );
        assert_eq!(
            key_action(&Key::parse("ctrl+shift+z").unwrap(), 34).unwrap(),
            KeyAction::Shell("input keycombination 113 59 54".into())
        );
        assert_eq!(
            key_action(&Key::parse("alt+tab").unwrap(), 33).unwrap(),
            KeyAction::Shell("input keycombination 57 61".into())
        );
        assert_eq!(
            key_action(&Key::parse("cmd+space").unwrap(), 33).unwrap(),
            KeyAction::Shell("input keycombination 117 62".into())
        );
        assert!(
            key_action(&Key::parse("ctrl+c").unwrap(), 30)
                .unwrap_err()
                .is_unsupported()
        );
        assert_eq!(keycode(&Key::Char('0')), Some((7, false)));
        assert_eq!(keycode(&Key::Char('9')), Some((16, false)));
        assert_eq!(keycode(&Key::Char('z')), Some((54, false)));
        assert_eq!(keycode(&Key::Char('?')), Some((76, true)));
        assert_eq!(keycode(&Key::Up), Some((19, false)));
        assert_eq!(keycode(&Key::Char('ü')), None);
        assert!(
            key_action(&Key::parse("ctrl+ü").unwrap(), 34)
                .unwrap_err()
                .is_unsupported()
        );
    }

    #[test]
    fn buttons() {
        assert_eq!(button_keycode(Button::Home), 3);
        assert_eq!(button_keycode(Button::Lock), 26);
        assert_eq!(button_keycode(Button::VolumeUp), 24);
        assert_eq!(button_keycode(Button::VolumeDown), 25);
        assert_eq!(button_keycode(Button::AppSwitcher), 187);
        assert_eq!(button_keycode(Button::Back), 4);
        assert_eq!(button_keycode(Button::Siri), 219);
    }

    #[test]
    fn apps_and_ui() {
        assert!(is_package_name("com.whatsapp"));
        assert!(is_package_name("org.mozilla.firefox_beta"));
        assert!(!is_package_name("com.x; reboot"));
        assert!(!is_package_name("com..x"));
        assert!(!is_package_name(""));
        assert_eq!(
            parse_packages("package:com.b\r\npackage:com.a\nnoise\npackage:com.a\n"),
            vec!["com.a", "com.b"]
        );
        let dump = "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation=\"0\"><node/></hierarchy>UI hierchary dumped to: /dev/tty\n";
        assert_eq!(
            extract_ui_xml(dump).unwrap(),
            "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation=\"0\"><node/></hierarchy>"
        );
        assert_eq!(
            extract_ui_xml("ERROR: null root node returned by UiTestAutomationBridge."),
            None
        );
        assert!(shell_error("Error: Unknown command: keycombination").is_some());
        assert!(shell_error("").is_none());
        assert!(shell_error("Events injected: 1").is_none());
    }

    #[test]
    fn foreground_package() {
        let t = "  mCurrentFocus=Window{1a2b u0 com.android.settings/com.android.settings.Settings}
  mFocusedApp=ActivityRecord{9 u0 com.android.settings/.Settings t12}
";
        assert_eq!(
            parse_foreground_package(t).as_deref(),
            Some("com.android.settings")
        );
        let shade = "  mCurrentFocus=Window{1 u0 NotificationShade}
  mFocusedApp=ActivityRecord{2 u0 com.google.android.apps.nexuslauncher/.NexusLauncherActivity t3}
";
        assert_eq!(
            parse_foreground_package(shade).as_deref(),
            Some("com.google.android.apps.nexuslauncher")
        );
        assert_eq!(
            parse_foreground_package(
                "mCurrentFocus=null
"
            ),
            None
        );
        assert_eq!(parse_foreground_package(""), None);
    }
}
