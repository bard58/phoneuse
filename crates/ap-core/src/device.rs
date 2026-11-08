//! The [`Device`] and [`DeviceProvider`] traits every backend implements.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Button, Error, Frame, Key, Point, Result, Size, SwipeDirection};

/// Phone operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Ios,
    Android,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Platform::Ios => "iOS",
            Platform::Android => "Android",
        })
    }
}

/// Static description of a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable id (iOS UDID, Android serial).
    pub id: String,
    /// User-visible name ("Ozan's iPhone").
    pub name: String,
    pub platform: Platform,
    pub os_version: String,
    /// Hardware model ("iPhone14,5", "Pixel 8").
    pub model: String,
    /// Logical size: UI points on iOS, dp (density-independent pixels) on Android.
    pub screen_points: Size,
    /// Physical screenshot size in pixels.
    pub screen_pixels: Size,
    /// Backend name ("hid", "wda", "adb", ...).
    pub backend: String,
}

/// What a connected device can do with its current backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub tap: bool,
    pub swipe: bool,
    pub type_text: bool,
    pub keys: bool,
    pub buttons: bool,
    pub launch_app: bool,
    pub ui_tree: bool,
    pub screenshot: bool,
}

impl Capabilities {
    /// Everything enabled.
    pub const ALL: Capabilities = Capabilities {
        tap: true,
        swipe: true,
        type_text: true,
        keys: true,
        buttons: true,
        launch_app: true,
        ui_tree: true,
        screenshot: true,
    };

    /// Nothing enabled.
    pub const NONE: Capabilities = Capabilities {
        tap: false,
        swipe: false,
        type_text: false,
        keys: false,
        buttons: false,
        launch_app: false,
        ui_tree: false,
        screenshot: false,
    };

    /// True when the device can send touch input (tap or swipe).
    pub fn has_touch(&self) -> bool {
        self.tap || self.swipe
    }

    /// Names of the enabled capabilities.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.tap, "tap"),
            (self.swipe, "swipe"),
            (self.type_text, "type_text"),
            (self.keys, "keys"),
            (self.buttons, "buttons"),
            (self.launch_app, "launch_app"),
            (self.ui_tree, "ui_tree"),
            (self.screenshot, "screenshot"),
        ]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect()
    }
}

/// A connected phone. All coordinates are normalized [`Point`]s.
#[async_trait]
pub trait Device: Send + Sync {
    fn info(&self) -> &DeviceInfo;

    fn capabilities(&self) -> Capabilities;

    async fn screenshot(&self) -> Result<Frame>;

    async fn tap(&self, at: Point) -> Result<()>;

    async fn double_tap(&self, at: Point) -> Result<()>;

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()>;

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()>;

    /// Scroll around `at`. `amount` is the fraction of the screen to move (0..1).
    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()>;

    /// Type text into the focused field.
    async fn type_text(&self, text: &str) -> Result<()>;

    async fn key(&self, key: &Key) -> Result<()>;

    async fn button(&self, button: Button) -> Result<()>;

    /// Open an app by iOS bundle id or Android package name.
    async fn launch_app(&self, bundle_or_package: &str) -> Result<()>;

    /// Installed app ids.
    async fn list_apps(&self) -> Result<Vec<String>> {
        Err(Error::unsupported("list_apps"))
    }

    /// Accessibility / UI hierarchy as text (XML or JSON), if the backend has one.
    async fn ui_tree(&self) -> Result<Option<String>> {
        Err(Error::unsupported("ui_tree"))
    }

    /// The app in the foreground (bundle id or package name), if the backend
    /// knows it. `Ok(None)` = unknown. Privacy guards treat unknown as unsafe.
    async fn foreground_app(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// Go to the home screen.
    async fn home(&self) -> Result<()> {
        self.button(Button::Home).await
    }

    /// Stop the calls that were sent before now, as far as the backend can.
    /// A host calls this when the owner cancels (for example Ctrl+C), in
    /// addition to dropping the call's future. Calls sent after this run
    /// normally. The default does nothing: the backend then stops only when
    /// the dropped future stops it.
    fn cancel(&self) {}
}

/// Finds and connects devices of one kind (iOS over usbmuxd, Android over adb, ...).
#[async_trait]
pub trait DeviceProvider: Send + Sync {
    /// Short provider name ("ios", "android").
    fn name(&self) -> &str;

    /// List reachable devices. Errors here should be [`Error::Setup`] with a fix hint.
    async fn discover(&self) -> Result<Vec<DeviceInfo>>;

    /// Connect to the device with this id.
    async fn connect(&self, id: &str) -> Result<Box<dyn Device>>;
}

/// The first iOS major version with touch input over CoreDevice HID. iOS
/// 17.4 to 26.x refuse the screen stream that touch input needs (CoreDevice
/// error 9021), but they give screenshots, buttons and keys.
pub const IOS_TOUCH_MIN_MAJOR: u16 = 27;

/// The fix text for an iPhone that is older than [`IOS_TOUCH_MIN_MAJOR`].
pub const IOS_UPDATE_HINT: &str = "Update the iPhone in Settings > General > Software Update.";

/// The message for an iPhone with iOS 27 or later that turned touch off at
/// run time (the screen stream or the touch service did not open).
pub const IOS_TOUCH_OFF_MESSAGE: &str = "Touch input is off: the iPhone did not open the screen stream or the touch service that touch needs. Buttons and keys can still work. Run `agentphone doctor` to mount the Developer Disk Image again.";

/// The problem text for an iPhone with iOS `os_version` and no touch input.
pub fn ios_touch_problem(os_version: &str) -> String {
    format!(
        "Touch input needs iOS {IOS_TOUCH_MIN_MAJOR} or later. This iPhone has iOS {os_version}."
    )
}

/// The full message for an iPhone with iOS `os_version` and no touch input.
pub fn ios_touch_message(os_version: &str) -> String {
    format!("{} {IOS_UPDATE_HINT}", ios_touch_problem(os_version))
}

/// The typed error for a touch call on an iPhone below iOS 27: an
/// [`Error::Setup`] with the update fix. It is not a transport error, so no
/// layer runs the call again.
pub fn ios_touch_error(os_version: &str) -> Error {
    Error::setup(ios_touch_problem(os_version), IOS_UPDATE_HINT)
}

/// The major number of a version text such as `26.3.1`, or `None`.
fn major_of(os_version: &str) -> Option<u16> {
    let digits: String = os_version
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Why the device has no touch input, or `None` when it has touch. Hosts
/// (the agent loop, MCP `status`, the CLI) show this text and do not send
/// taps that cannot work.
pub fn touch_unavailable(info: &DeviceInfo, caps: Capabilities) -> Option<String> {
    if caps.has_touch() {
        return None;
    }
    let ios_major = (info.platform == Platform::Ios)
        .then(|| major_of(&info.os_version))
        .flatten();
    Some(match ios_major {
        Some(m) if m < IOS_TOUCH_MIN_MAJOR => ios_touch_message(&info.os_version),
        // iOS 27 or later: the backend turned touch off at run time.
        Some(_) => IOS_TOUCH_OFF_MESSAGE.to_string(),
        None => format!(
            "This {} device has no touch input with the {} backend.",
            info.platform, info.backend
        ),
    })
}

/// Validate a point before sending it to a backend.
pub fn check_point(p: Point) -> Result<Point> {
    if p.is_valid() {
        Ok(p)
    } else {
        Err(Error::InvalidArgument(format!(
            "point ({}, {}) is outside 0..1",
            p.x, p.y
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_names() {
        assert_eq!(Capabilities::ALL.names().len(), 8);
        assert!(Capabilities::NONE.names().is_empty());
        assert!(check_point(Point::new(2.0, 0.0)).is_err());
        assert_eq!(Platform::Ios.to_string(), "iOS");
    }

    fn info(platform: Platform, os: &str) -> DeviceInfo {
        DeviceInfo {
            id: "x".into(),
            name: "x".into(),
            platform,
            os_version: os.into(),
            model: "iPhone14,5".into(),
            screen_points: Size::new(390.0, 844.0),
            screen_pixels: Size::new(1170.0, 2532.0),
            backend: "hid".into(),
        }
    }

    #[test]
    fn touch_unavailable_message() {
        let screen_only = Capabilities {
            tap: false,
            swipe: false,
            ..Capabilities::ALL
        };
        assert!(Capabilities::ALL.has_touch());
        assert!(!screen_only.has_touch());
        assert_eq!(
            touch_unavailable(&info(Platform::Ios, "27.0"), Capabilities::ALL),
            None
        );
        let m = touch_unavailable(&info(Platform::Ios, "26.3"), screen_only).unwrap();
        assert_eq!(
            m,
            "Touch input needs iOS 27 or later. This iPhone has iOS 26.3. Update the iPhone in Settings > General > Software Update."
        );
        // iOS 27 with touch turned off at run time: no update message.
        let m = touch_unavailable(&info(Platform::Ios, "27.0.1"), screen_only).unwrap();
        assert_eq!(m, IOS_TOUCH_OFF_MESSAGE);
        let m = touch_unavailable(&info(Platform::Android, "15"), screen_only).unwrap();
        assert!(m.contains("no touch input"), "{m}");
        let e = ios_touch_error("18.7.3");
        assert!(e.to_string().contains("This iPhone has iOS 18.7.3."), "{e}");
        assert_eq!(e.hint(), Some(IOS_UPDATE_HINT));
        assert_eq!(major_of("26.0b2"), Some(26));
        assert_eq!(major_of("beta"), None);
    }
}
