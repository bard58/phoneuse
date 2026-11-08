//! A fake [`Device`] that records every call. For tests in other crates.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::device::check_point;
use crate::{
    Button, Capabilities, Device, DeviceInfo, Error, Frame, Gesture, Key, Platform, Point, Result,
    Size, SwipeDirection,
};

/// Recording mock device. Clones share the same call log.
#[derive(Debug, Clone)]
pub struct MockDevice {
    info: DeviceInfo,
    caps: Capabilities,
    color: [u8; 4],
    calls: Arc<Mutex<Vec<Gesture>>>,
    screenshots: Arc<Mutex<u32>>,
    apps: Vec<String>,
    ui_tree: Option<String>,
}

impl Default for MockDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDevice {
    /// An "iPhone 13" (390x844 pt, 1170x2532 px) with all capabilities and a grey screen.
    pub fn new() -> Self {
        Self {
            info: DeviceInfo {
                id: "mock-0001".into(),
                name: "Mock iPhone".into(),
                platform: Platform::Ios,
                os_version: "26.0".into(),
                model: "iPhone14,5".into(),
                screen_points: Size::new(390.0, 844.0),
                screen_pixels: Size::new(1170.0, 2532.0),
                backend: "mock".into(),
            },
            caps: Capabilities::ALL,
            color: [128, 128, 128, 255],
            calls: Arc::default(),
            screenshots: Arc::default(),
            apps: vec![
                "com.apple.Preferences".into(),
                "com.apple.mobilesafari".into(),
            ],
            ui_tree: None,
        }
    }

    pub fn with_info(mut self, info: DeviceInfo) -> Self {
        self.info = info;
        self
    }

    pub fn with_capabilities(mut self, caps: Capabilities) -> Self {
        self.caps = caps;
        self
    }

    pub fn with_color(mut self, rgba: [u8; 4]) -> Self {
        self.color = rgba;
        self
    }

    pub fn with_ui_tree(mut self, tree: impl Into<String>) -> Self {
        self.ui_tree = Some(tree.into());
        self
    }

    /// All recorded actions so far (screenshots are not recorded here).
    pub fn calls(&self) -> Vec<Gesture> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Number of screenshots taken.
    pub fn screenshot_count(&self) -> u32 {
        self.screenshots.lock().map(|c| *c).unwrap_or(0)
    }

    pub fn clear(&self) {
        if let Ok(mut c) = self.calls.lock() {
            c.clear();
        }
    }

    fn record(&self, enabled: bool, what: &str, g: Gesture) -> Result<()> {
        if !enabled {
            return Err(Error::unsupported(what));
        }
        self.calls
            .lock()
            .map_err(|_| Error::Other("mock lock poisoned".into()))?
            .push(g);
        Ok(())
    }
}

#[async_trait]
impl Device for MockDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn screenshot(&self) -> Result<Frame> {
        if !self.caps.screenshot {
            return Err(Error::unsupported("screenshot"));
        }
        if let Ok(mut n) = self.screenshots.lock() {
            *n += 1;
        }
        let s = self.info.screen_pixels;
        Ok(Frame::solid(s.width as u32, s.height as u32, self.color))
    }

    async fn tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.record(self.caps.tap, "tap", Gesture::Tap { at })
    }

    async fn double_tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.record(self.caps.tap, "double_tap", Gesture::DoubleTap { at })
    }

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()> {
        let at = check_point(at)?;
        self.record(
            self.caps.tap,
            "long_press",
            Gesture::LongPress { at, duration },
        )
    }

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let (from, to) = (check_point(from)?, check_point(to)?);
        self.record(
            self.caps.swipe,
            "swipe",
            Gesture::Swipe { from, to, duration },
        )
    }

    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()> {
        let at = check_point(at)?;
        self.record(
            self.caps.swipe,
            "scroll",
            Gesture::Scroll {
                at,
                direction,
                amount,
            },
        )
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.record(
            self.caps.type_text,
            "type_text",
            Gesture::TypeText { text: text.into() },
        )
    }

    async fn key(&self, key: &Key) -> Result<()> {
        self.record(self.caps.keys, "key", Gesture::Key { key: key.clone() })
    }

    async fn button(&self, button: Button) -> Result<()> {
        self.record(self.caps.buttons, "button", Gesture::Button { button })
    }

    async fn launch_app(&self, bundle_or_package: &str) -> Result<()> {
        self.record(
            self.caps.launch_app,
            "launch_app",
            Gesture::LaunchApp {
                id: bundle_or_package.into(),
            },
        )
    }

    async fn list_apps(&self) -> Result<Vec<String>> {
        Ok(self.apps.clone())
    }

    async fn ui_tree(&self) -> Result<Option<String>> {
        if self.caps.ui_tree {
            Ok(self.ui_tree.clone())
        } else {
            Err(Error::unsupported("ui_tree"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_calls() {
        let m = MockDevice::new().with_color([255, 0, 0, 255]);
        let d: Box<dyn Device> = Box::new(m.clone());
        d.tap(Point::CENTER).await.unwrap();
        d.home().await.unwrap();
        d.key(&Key::Enter).await.unwrap();
        assert!(d.tap(Point::new(3.0, 0.0)).await.is_err());
        let f = d.screenshot().await.unwrap();
        assert_eq!((f.width, f.height), (1170, 2532));
        assert_eq!(f.image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(
            m.calls(),
            vec![
                Gesture::Tap { at: Point::CENTER },
                Gesture::Button {
                    button: Button::Home
                },
                Gesture::Key { key: Key::Enter },
            ]
        );
        assert_eq!(m.screenshot_count(), 1);
    }

    #[tokio::test]
    async fn respects_capabilities() {
        let m = MockDevice::new().with_capabilities(Capabilities {
            tap: true,
            ..Capabilities::NONE
        });
        assert!(m.tap(Point::CENTER).await.is_ok());
        assert!(
            m.swipe(Point::CENTER, Point::new(0.5, 0.1), Duration::ZERO)
                .await
                .unwrap_err()
                .is_unsupported()
        );
        assert!(m.ui_tree().await.is_err());
    }
}
