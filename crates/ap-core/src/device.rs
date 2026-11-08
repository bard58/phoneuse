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
    /// Logical size in UI points (iOS) or pixels (Android).
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

    /// Go to the home screen.
    async fn home(&self) -> Result<()> {
        self.button(Button::Home).await
    }
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
}
