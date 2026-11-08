//! [`AndroidDevice`]: the [`Device`] implementation over a [`Transport`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ap_core::device::check_point;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, Error, Frame, Key, Platform, Point, Result, Size,
    SwipeDirection,
};
use async_trait::async_trait;

use crate::adb::Transport;
use crate::parse::{self, KeyAction};

/// Default duration of a scroll gesture.
const SCROLL_DURATION: Duration = Duration::from_millis(300);
/// Shortest long press Android reliably treats as one.
const MIN_LONG_PRESS: Duration = Duration::from_millis(500);

/// One Android device reached through adb.
pub struct AndroidDevice {
    transport: Arc<dyn Transport>,
    serial: String,
    info: DeviceInfo,
    sdk: u32,
    /// Current screen size in pixels. Updated from each screenshot, so a
    /// rotated screen maps taps correctly after the next screenshot.
    pixels: Mutex<Size>,
}

impl std::fmt::Debug for AndroidDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidDevice")
            .field("serial", &self.serial)
            .field("info", &self.info)
            .field("sdk", &self.sdk)
            .finish()
    }
}

impl AndroidDevice {
    /// Query the device (model, version, screen) and build a handle.
    pub async fn open(transport: Arc<dyn Transport>, serial: &str) -> Result<Self> {
        let (info, sdk) = query_info(transport.as_ref(), serial).await?;
        Ok(Self::with_info(transport, serial, info, sdk))
    }

    /// Build a handle from known info (no device round trip).
    pub fn with_info(
        transport: Arc<dyn Transport>,
        serial: &str,
        info: DeviceInfo,
        sdk: u32,
    ) -> Self {
        Self {
            pixels: Mutex::new(info.screen_pixels),
            transport,
            serial: serial.to_string(),
            info,
            sdk,
        }
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Android API level (0 if unknown).
    pub fn sdk(&self) -> u32 {
        self.sdk
    }

    /// Run a shell command and return stdout+stderr as text.
    pub async fn shell(&self, command: &str) -> Result<String> {
        let out = self.transport.exec(&self.serial, command).await?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Run an `input`/`am` style command; fail if it printed an error.
    async fn run(&self, command: &str) -> Result<()> {
        let out = self.shell(&format!("{command} 2>&1")).await?;
        match parse::shell_error(&out) {
            Some(e) => Err(Error::protocol(format!("`{command}` failed: {e}"))),
            None => Ok(()),
        }
    }

    fn pixels(&self) -> Size {
        *self.pixels.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn px(&self, p: Point) -> Result<(u32, u32)> {
        let size = self.pixels();
        if size.is_empty() {
            return Err(Error::protocol(
                "screen size unknown; take a screenshot first",
            ));
        }
        Ok(parse::to_px(check_point(p)?, size))
    }

    async fn type_unicode(&self, text: &str) -> Result<()> {
        let ime = self
            .shell("settings get secure default_input_method")
            .await
            .unwrap_or_default();
        if !ime.contains(parse::ADB_KEYBOARD_PKG) {
            return Err(Error::setup(
                "this text has non-ASCII characters, which `adb shell input text` cannot type",
                format!(
                    "install ADBKeyBoard (https://github.com/senzhk/ADBKeyBoard) on the phone \
                     and make it the active keyboard (`adb shell ime enable {ime}` then \
                     `adb shell ime set {ime}`), or type ASCII only",
                    ime = parse::ADB_KEYBOARD_IME
                ),
            ));
        }
        self.run(&parse::adb_keyboard_script(text)).await
    }
}

/// Read model, version and screen size in one round trip.
pub async fn query_info(transport: &dyn Transport, serial: &str) -> Result<(DeviceInfo, u32)> {
    let out = transport.exec(serial, parse::INFO_SCRIPT).await?;
    let p = parse::parse_info(&String::from_utf8_lossy(&out));
    let (w, h) = p.pixels.ok_or_else(|| {
        Error::protocol(format!(
            "cannot read the screen size of {serial} (`wm size` gave no answer)"
        ))
    })?;
    let pixels = Size::new(w as f32, h as f32);
    let points = parse::pixels_to_points(pixels, p.dpi.unwrap_or(160));
    let info = DeviceInfo {
        id: serial.to_string(),
        name: parse::display_name(&p.manufacturer, &p.model),
        platform: Platform::Android,
        os_version: p.release,
        model: p.model,
        screen_points: points,
        screen_pixels: pixels,
        backend: "adb".into(),
    };
    Ok((info, p.sdk))
}

#[async_trait]
impl Device for AndroidDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    async fn screenshot(&self) -> Result<Frame> {
        let png = self.transport.exec(&self.serial, "screencap -p").await?;
        if !png.starts_with(b"\x89PNG") {
            let text = String::from_utf8_lossy(&png[..png.len().min(200)]).into_owned();
            return Err(Error::protocol(format!(
                "screencap did not return a PNG: {}",
                text.trim()
            )));
        }
        let frame = Frame::from_encoded(&png)?;
        *self.pixels.lock().unwrap_or_else(|e| e.into_inner()) = frame.size();
        Ok(frame)
    }

    async fn tap(&self, at: Point) -> Result<()> {
        let (x, y) = self.px(at)?;
        self.run(&format!("input tap {x} {y}")).await
    }

    async fn double_tap(&self, at: Point) -> Result<()> {
        let (x, y) = self.px(at)?;
        // Each `input` call starts a JVM (~100-300 ms), so start the first tap in
        // the background to land both inside the double-tap timeout.
        self.run(&format!(
            "input tap {x} {y} & sleep 0.08; input tap {x} {y}; wait"
        ))
        .await
    }

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()> {
        let (x, y) = self.px(at)?;
        let ms = parse::duration_ms(duration.max(MIN_LONG_PRESS));
        self.run(&format!("input swipe {x} {y} {x} {y} {ms}")).await
    }

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let (x1, y1) = self.px(from)?;
        let (x2, y2) = self.px(to)?;
        let ms = parse::duration_ms(duration);
        self.run(&format!("input swipe {x1} {y1} {x2} {y2} {ms}"))
            .await
    }

    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()> {
        let at = check_point(at)?;
        let (from, to) = parse::scroll_points(at, direction, amount);
        self.swipe(from, to, SCROLL_DURATION).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if !parse::is_input_text_safe(text) {
            return self.type_unicode(text).await;
        }
        for script in parse::text_scripts(text)? {
            self.run(&script).await?;
        }
        Ok(())
    }

    async fn key(&self, key: &Key) -> Result<()> {
        match parse::key_action(key, self.sdk)? {
            KeyAction::Text(t) => self.type_text(&t).await,
            KeyAction::Shell(cmd) => self.run(&cmd).await,
        }
    }

    async fn button(&self, button: Button) -> Result<()> {
        self.run(&format!("input keyevent {}", parse::button_keycode(button)))
            .await
    }

    async fn launch_app(&self, package: &str) -> Result<()> {
        if !parse::is_package_name(package) {
            return Err(Error::InvalidArgument(format!(
                "{package:?} is not an Android package name (e.g. com.android.settings)"
            )));
        }
        let out = self
            .shell(&format!(
                "monkey -p {package} -c android.intent.category.LAUNCHER 1 2>&1"
            ))
            .await?;
        if out.contains("No activities found") || out.contains("monkey aborted") {
            return Err(Error::InvalidArgument(format!(
                "package {package} is not installed or has no launcher activity \
                 (see list_apps)"
            )));
        }
        if let Some(e) = parse::shell_error(&out) {
            return Err(Error::protocol(format!("launching {package} failed: {e}")));
        }
        Ok(())
    }

    async fn list_apps(&self) -> Result<Vec<String>> {
        let out = self.shell("pm list packages -3 2>&1").await?;
        Ok(parse::parse_packages(&out))
    }

    async fn foreground_app(&self) -> Result<Option<String>> {
        let out = self
            .shell("dumpsys window 2>&1 | grep -E 'mCurrentFocus|mFocusedApp'")
            .await?;
        Ok(parse::parse_foreground_package(&out))
    }

    async fn ui_tree(&self) -> Result<Option<String>> {
        let out = self.shell("uiautomator dump /dev/tty 2>&1").await?;
        if let Some(xml) = parse::extract_ui_xml(&out) {
            return Ok(Some(xml));
        }
        // Some builds cannot write to /dev/tty; fall back to a temp file.
        let path = "/data/local/tmp/agentphone_ui.xml";
        let out = self
            .shell(&format!(
                "uiautomator dump {path} >/dev/null 2>&1; cat {path} 2>&1; rm -f {path}"
            ))
            .await?;
        match parse::extract_ui_xml(&out) {
            Some(xml) => Ok(Some(xml)),
            None => Err(Error::protocol(format!(
                "uiautomator dump failed: {}",
                out.trim().lines().next().unwrap_or("no output")
            ))),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Records commands and replies from a queue (or a default reply).
    #[derive(Default)]
    pub(crate) struct FakeTransport {
        pub calls: Mutex<Vec<String>>,
        pub replies: Mutex<VecDeque<Vec<u8>>>,
        pub host_reply: Mutex<String>,
    }

    impl FakeTransport {
        pub fn reply(&self, r: impl Into<Vec<u8>>) {
            self.replies.lock().unwrap().push_back(r.into());
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn host(&self, service: &str) -> Result<String> {
            self.calls.lock().unwrap().push(format!("host:{service}"));
            Ok(self.host_reply.lock().unwrap().clone())
        }
        async fn exec(&self, serial: &str, command: &str) -> Result<Vec<u8>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{serial}$ {command}"));
            Ok(self.replies.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    pub(crate) const INFO_REPLY: &str = "Google\nPixel 8\n14\n34\n@@AP@@\nPhysical size: 1080x2400\n@@AP@@\nPhysical density: 420\n";

    async fn device() -> (Arc<FakeTransport>, AndroidDevice) {
        let t = Arc::new(FakeTransport::default());
        t.reply(INFO_REPLY);
        let d = AndroidDevice::open(t.clone(), "SER1").await.unwrap();
        t.calls.lock().unwrap().clear();
        (t, d)
    }

    #[tokio::test]
    async fn info_from_device() {
        let (_, d) = device().await;
        let i = d.info();
        assert_eq!(i.name, "Google Pixel 8");
        assert_eq!(i.os_version, "14");
        assert_eq!(i.screen_pixels, Size::new(1080.0, 2400.0));
        assert_eq!(i.screen_points, Size::new(411.0, 914.0));
        assert_eq!(i.platform, Platform::Android);
        assert_eq!(d.sdk(), 34);
    }

    #[tokio::test]
    async fn gestures_send_pixels() {
        let (t, d) = device().await;
        d.tap(Point::new(0.5, 0.25)).await.unwrap();
        d.long_press(Point::CENTER, Duration::from_millis(100))
            .await
            .unwrap();
        d.swipe(
            Point::new(0.5, 0.8),
            Point::new(0.5, 0.2),
            Duration::from_millis(250),
        )
        .await
        .unwrap();
        d.scroll(Point::CENTER, SwipeDirection::Up, 0.5)
            .await
            .unwrap();
        d.button(Button::Home).await.unwrap();
        assert!(d.tap(Point::new(1.5, 0.0)).await.is_err());
        assert_eq!(
            t.calls(),
            vec![
                "SER1$ input tap 540 600 2>&1",
                "SER1$ input swipe 540 1200 540 1200 500 2>&1",
                "SER1$ input swipe 540 1920 540 480 250 2>&1",
                "SER1$ input swipe 540 1800 540 600 300 2>&1",
                "SER1$ input keyevent 3 2>&1",
            ]
        );
    }

    #[tokio::test]
    async fn text_and_keys() {
        let (t, d) = device().await;
        d.type_text("it's ok").await.unwrap();
        d.key(&Key::Enter).await.unwrap();
        d.key(&Key::parse("ctrl+a").unwrap()).await.unwrap();
        assert_eq!(
            t.calls(),
            vec![
                "SER1$ input text 'it'\\''s%sok' 2>&1",
                "SER1$ input keyevent 66 2>&1",
                "SER1$ input keycombination 113 29 2>&1",
            ]
        );
        // Unicode without ADBKeyBoard -> setup error with a hint.
        t.reply("com.google.android.inputmethod.latin/.LatinIME\n");
        let e = d.type_text("çay").await.unwrap_err();
        assert!(e.hint().unwrap().contains("ADBKeyBoard"));
        // With ADBKeyBoard active -> broadcast.
        t.reply("com.android.adbkeyboard/.AdbIME\n");
        d.type_text("çay").await.unwrap();
        assert!(t.calls().last().unwrap().contains("ADB_INPUT_B64"));
    }

    #[tokio::test]
    async fn input_errors_surface() {
        let (t, d) = device().await;
        t.reply("Error: Unknown command: keycombination\nUsage: input ...");
        assert!(d.key(&Key::parse("ctrl+a").unwrap()).await.is_err());
    }

    #[tokio::test]
    async fn screenshot_updates_size() {
        let (t, d) = device().await;
        let png = Frame::solid(24, 12, [1, 2, 3, 255]).to_png().unwrap();
        t.reply(png);
        let f = d.screenshot().await.unwrap();
        assert_eq!((f.width, f.height), (24, 12));
        d.tap(Point::CENTER).await.unwrap();
        assert_eq!(t.calls()[1], "SER1$ input tap 12 6 2>&1");
        t.reply("Error: secure");
        assert!(d.screenshot().await.is_err());
    }

    #[tokio::test]
    async fn apps_and_tree() {
        let (t, d) = device().await;
        assert!(d.launch_app("x; reboot").await.is_err());
        t.reply("  bash arg: -p\n  bash arg: com.x\nEvents injected: 1\n");
        d.launch_app("com.x").await.unwrap();
        t.reply("** No activities found to run, monkey aborted.\n");
        assert!(d.launch_app("com.missing").await.is_err());
        t.reply("package:com.b\npackage:com.a\n");
        assert_eq!(d.list_apps().await.unwrap(), vec!["com.a", "com.b"]);
        t.reply(
            "<?xml version='1.0' ?><hierarchy><node/></hierarchy>UI hierchary dumped to: /dev/tty",
        );
        assert_eq!(
            d.ui_tree().await.unwrap().unwrap(),
            "<?xml version='1.0' ?><hierarchy><node/></hierarchy>"
        );
        t.reply("ERROR: could not get idle state.");
        t.reply("<hierarchy><node/></hierarchy>");
        assert_eq!(
            d.ui_tree().await.unwrap().unwrap(),
            "<hierarchy><node/></hierarchy>"
        );
        assert!(
            t.calls()
                .iter()
                .any(|c| c.contains("monkey -p com.x -c android.intent.category.LAUNCHER 1"))
        );
    }
}
