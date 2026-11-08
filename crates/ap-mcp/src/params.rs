//! Tool argument types. Field doc comments become the JSON schema descriptions.

use ap_core::{Button, SwipeDirection};
use schemars::JsonSchema;
use serde::Deserialize;

/// What an action returns about the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    #[default]
    Strip,
    Both,
    Text,
    Image,
    None,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct LookArgs {
    /// "both" (default): one settled screenshot plus OCR text lines. "strip": frames at
    /// +0.1/+0.4/+0.8 s to watch something move. "text": text lines only. "image": image only.
    #[serde(default)]
    pub look: Option<Look>,
    /// Draw a labelled 0.1 coordinate grid on the image to help read positions.
    #[serde(default)]
    pub grid: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WaitArgs {
    /// Seconds to wait, 0 to 10 (default 1).
    #[serde(default)]
    pub seconds: Option<f32>,
    /// Screen to return afterwards (default "strip").
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TapArgs {
    /// Normalized x: fraction of the screen width, 0.0 (left) to 1.0 (right).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x: f32,
    /// Normalized y: fraction of the screen height, 0.0 (top) to 1.0 (bottom).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y: f32,
    /// Tap twice quickly (zoom a photo or map, select a word). Default false.
    #[serde(default)]
    pub double: Option<bool>,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TapOnArgs {
    /// The visible text to tap, e.g. "Wi-Fi" or "General". Case-insensitive;
    /// words like "button", "row", "icon" are ignored.
    pub target: String,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LongPressArgs {
    /// Normalized x, 0.0 (left) to 1.0 (right).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x: f32,
    /// Normalized y, 0.0 (top) to 1.0 (bottom).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y: f32,
    /// How long to hold, 0.1 to 5 seconds (default 0.8).
    #[serde(default)]
    pub seconds: Option<f32>,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SwipeArgs {
    /// Start x, normalized 0.0 to 1.0.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x0: f32,
    /// Start y, normalized 0.0 to 1.0.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y0: f32,
    /// End x, normalized 0.0 to 1.0.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x1: f32,
    /// End y, normalized 0.0 to 1.0.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y1: f32,
    /// Duration of the drag, 0.05 to 5 seconds (default 0.3).
    #[serde(default)]
    pub seconds: Option<f32>,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

/// Where the content should move: "down" shows what is below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    #[default]
    Down,
    Left,
    Right,
}

impl Direction {
    /// The finger moves the opposite way to the content you want to see.
    pub fn finger(self) -> SwipeDirection {
        match self {
            Direction::Down => SwipeDirection::Up,
            Direction::Up => SwipeDirection::Down,
            Direction::Right => SwipeDirection::Left,
            Direction::Left => SwipeDirection::Right,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ScrollArgs {
    /// "down" (default) shows what is below, "up" what is above; "left"/"right" for carousels.
    #[serde(default)]
    pub direction: Option<Direction>,
    /// How far, in half-screens (default 1.0 = half a screen, max 2.0 = a full screen).
    #[serde(default)]
    pub amount: Option<f32>,
    /// Optional normalized x (0.0 to 1.0) of the area to scroll; default is the centre.
    #[serde(default)]
    pub x: Option<f32>,
    /// Optional normalized y (0.0 to 1.0) of the area to scroll; default is the centre.
    #[serde(default)]
    pub y: Option<f32>,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TypeTextArgs {
    /// The text to type. "\n" presses Enter (which can send a message).
    pub text: String,
    /// Replace what the field already holds (select all first). Default true;
    /// false adds to it.
    #[serde(default)]
    pub replace: Option<bool>,
    /// Text of 4 to 8 digits looks like a passcode and is refused. Set true
    /// only when the user asked in chat for exactly these digits and they are
    /// not a passcode, PIN or password.
    #[serde(default)]
    pub user_confirmed_not_passcode: Option<bool>,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PressKeyArgs {
    /// A key or shortcut: enter, escape, backspace, delete, tab, space, up, down,
    /// left, right, a single character, or combos like cmd+a, ctrl+shift+left.
    pub combo: String,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ButtonName {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    AppSwitcher,
    Back,
}

impl From<ButtonName> for Button {
    fn from(b: ButtonName) -> Self {
        match b {
            ButtonName::Home => Button::Home,
            ButtonName::Lock => Button::Lock,
            ButtonName::VolumeUp => Button::VolumeUp,
            ButtonName::VolumeDown => Button::VolumeDown,
            ButtonName::AppSwitcher => Button::AppSwitcher,
            ButtonName::Back => Button::Back,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ButtonArgs {
    /// The button: home, lock, volume_up, volume_down, app_switcher or back.
    pub name: ButtonName,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OpenAppArgs {
    /// App name ("Settings", "Safari") or iOS bundle id / Android package name
    /// ("com.apple.Preferences", "com.android.settings").
    pub name: String,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavTarget {
    Home,
    Back,
    #[serde(alias = "switcher")]
    AppSwitcher,
    #[serde(alias = "control-center")]
    ControlCenter,
    Notifications,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NavigateArgs {
    /// home, back (swipe in from the left edge), app_switcher, control_center
    /// (iPhone; quick settings on Android) or notifications.
    pub to: NavTarget,
    /// Screen to return: "strip" (default), "both", "text", "image" or "none".
    #[serde(default)]
    pub look: Option<Look>,
}
