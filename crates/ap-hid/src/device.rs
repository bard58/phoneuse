//! [`HidDevice`]: an ap-core [`Device`] that gives input through the dongle.
//!
//! The dongle has no screen. Pair it with a screen source (ap-ios DVT
//! screenshots, a future AirPlay receiver) with [`HidDevice::with_screen`].

use std::sync::Arc;
use std::time::Duration;

use ap_core::device::check_point;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, DeviceProvider, Error, Frame, Key, Platform, Point,
    Result, Size, SwipeDirection,
};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::calibration::CalibrationProfile;
use crate::discover::discover;
use crate::gesture::{self, Plan, Step, SystemGesture, TAP_HOLD, Touch};
use crate::protocol::DongleInfo;
use crate::report::key_usage;
use crate::transport::{Link, MemoryTransport, SerialTransport, SimHandle, Transport};

/// After this long without an absolute report, the pointer position is treated
/// as unknown (AssistiveTouch may hide it, the user may move it) and the next
/// press waits the longer settle time.
pub const POINTER_STALE: Duration = Duration::from_secs(5);
/// Swipe duration used by [`Device::scroll`].
pub const SCROLL_DURATION: Duration = Duration::from_millis(350);

/// Something that can take screenshots of the phone.
#[async_trait]
pub trait ScreenSource: Send + Sync {
    async fn screenshot(&self) -> Result<Frame>;

    /// Screenshot size in pixels, if known.
    fn screen_pixels(&self) -> Option<Size> {
        None
    }
}

#[async_trait]
impl ScreenSource for Box<dyn Device> {
    async fn screenshot(&self) -> Result<Frame> {
        self.as_ref().screenshot().await
    }

    fn screen_pixels(&self) -> Option<Size> {
        let s = self.info().screen_pixels;
        (!s.is_empty()).then_some(s)
    }
}

#[async_trait]
impl ScreenSource for Arc<dyn Device> {
    async fn screenshot(&self) -> Result<Frame> {
        self.as_ref().screenshot().await
    }

    fn screen_pixels(&self) -> Option<Size> {
        let s = self.info().screen_pixels;
        (!s.is_empty()).then_some(s)
    }
}

struct Inner {
    link: Link,
    /// Last absolute position sent, and when.
    pointer: Option<((f64, f64), Instant)>,
}

/// Input through an AgentPhone HID dongle (iOS AssistiveTouch).
pub struct HidDevice {
    info: DeviceInfo,
    profile: CalibrationProfile,
    dongle: DongleInfo,
    inner: Mutex<Inner>,
    screen: Option<Box<dyn ScreenSource>>,
}

impl std::fmt::Debug for HidDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HidDevice")
            .field("info", &self.info)
            .field("profile", &self.profile.name)
            .field("dongle", &self.dongle)
            .field("screen", &self.screen.is_some())
            .finish()
    }
}

impl HidDevice {
    /// Open the dongle on a serial port ("COM5") and do the handshake.
    pub async fn open(port: &str, profile: CalibrationProfile) -> Result<Self> {
        let t = SerialTransport::open(port)?;
        Self::connect(Box::new(t), profile).await
    }

    /// Handshake over any transport.
    pub async fn connect(
        transport: Box<dyn Transport>,
        profile: CalibrationProfile,
    ) -> Result<Self> {
        let mut link = Link::new(transport);
        let dongle = link.hello().await?;
        if !profile.measured {
            tracing::warn!(
                "calibration profile '{}' is not measured; drags may land short or long. \
                 See ap_hid::calibration::CALIBRATION_PROCEDURE",
                profile.name
            );
        }
        let port = link.port_name();
        let info = DeviceInfo {
            id: format!("hid:{port}"),
            name: format!("{} on {port}", dongle.name),
            platform: Platform::Ios,
            os_version: String::new(),
            model: profile.models.first().cloned().unwrap_or_default(),
            screen_points: profile.screen_points,
            screen_pixels: Size::default(),
            backend: "hid".into(),
        };
        Ok(Self {
            info,
            profile,
            dongle,
            inner: Mutex::new(Inner {
                link,
                pointer: None,
            }),
            screen: None,
        })
    }

    /// A device on the in-memory simulator, for tests.
    pub async fn simulated(profile: CalibrationProfile) -> Result<(Self, SimHandle)> {
        let (t, sim) = MemoryTransport::new();
        Ok((Self::connect(Box::new(t), profile).await?, sim))
    }

    /// Use `screen` for [`Device::screenshot`].
    pub fn with_screen(mut self, screen: Box<dyn ScreenSource>) -> Self {
        if let Some(px) = screen.screen_pixels() {
            self.info.screen_pixels = px;
        }
        self.screen = Some(screen);
        self
    }

    /// Use another device (for example an ap-ios device) only for screenshots.
    pub fn with_screen_device(self, device: Box<dyn Device>) -> Self {
        self.with_screen(Box::new(device))
    }

    /// Set the device info fields a screen source knows better (name, OS version, model).
    pub fn with_info(mut self, f: impl FnOnce(&mut DeviceInfo)) -> Self {
        f(&mut self.info);
        self
    }

    pub fn profile(&self) -> &CalibrationProfile {
        &self.profile
    }

    /// The dongle's HELLO_ACK.
    pub fn dongle(&self) -> &DongleInfo {
        &self.dongle
    }

    /// Ask the dongle whether a phone is connected to it.
    pub async fn phone_connected(&self) -> Result<bool> {
        self.inner.lock().await.link.ping().await
    }

    /// Release every key and button (safe to call any time).
    pub async fn release_all(&self) -> Result<()> {
        self.inner.lock().await.link.release_all().await
    }

    /// Run a planned gesture.
    pub async fn run_plan(&self, plan: &Plan) -> Result<()> {
        let mut inner = self.inner.lock().await;
        execute(&mut inner, plan).await
    }

    /// Plan with a [`Touch`] that starts from the tracked pointer, then run it.
    async fn touch(&self, f: impl FnOnce(&mut Touch<'_>)) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let start = inner
            .pointer
            .filter(|(_, at)| at.elapsed() < POINTER_STALE)
            .map(|(p, _)| p);
        let mut t = Touch::new(&self.profile, start);
        f(&mut t);
        let res = execute(&mut inner, &t.plan).await;
        if res.is_err() {
            // Never leave a finger down.
            inner.pointer = None;
            let _ = inner.link.release_all().await;
        }
        res
    }

    async fn keys(&self, f: impl FnOnce(&mut Plan) -> Result<()>) -> Result<()> {
        let mut plan = Plan::default();
        f(&mut plan)?;
        self.run_plan(&plan).await
    }

    /// System gesture (back, app switcher, Control Center, ...).
    pub async fn system(&self, g: SystemGesture) -> Result<()> {
        let (from, to, d, hold) = g.path();
        self.touch(|t| t.swipe(from, to, d, hold)).await
    }

    /// Back: edge swipe from the left edge.
    pub async fn back(&self) -> Result<()> {
        self.system(SystemGesture::Back).await
    }

    /// Show or hide the on-screen keyboard (Eject).
    pub async fn toggle_keyboard(&self) -> Result<()> {
        self.keys(|p| {
            gesture::eject(p);
            Ok(())
        })
        .await
    }

    /// One calibration stroke: press at the left middle, `count` reports of
    /// `(units, 0)`, release. See [`crate::calibration::CALIBRATION_PROCEDURE`].
    pub async fn calibration_stroke(&self, units: u8, count: u32) -> Result<()> {
        if units == 0 || units > 127 {
            return Err(Error::InvalidArgument("units must be 1..=127".into()));
        }
        self.touch(|t| t.calibration_stroke((0.1, 0.5), units, count))
            .await
    }
}

async fn execute(inner: &mut Inner, plan: &Plan) -> Result<()> {
    for step in &plan.steps {
        match step {
            Step::Send(r) => {
                inner.link.send_report(r).await?;
                if let crate::report::Report::Absolute { x, y, .. } = *r {
                    let p = (
                        crate::report::abs_fraction(x),
                        crate::report::abs_fraction(y),
                    );
                    inner.pointer = Some((p, Instant::now()));
                }
            }
            Step::Wait(d) => tokio::time::sleep(*d).await,
        }
    }
    Ok(())
}

fn xy(p: Point) -> (f64, f64) {
    (f64::from(p.x), f64::from(p.y))
}

#[async_trait]
impl Device for HidDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tap: true,
            swipe: true,
            type_text: true,
            keys: true,
            // Home and app switcher only; see Device::button.
            buttons: true,
            launch_app: true,
            ui_tree: false,
            screenshot: self.screen.is_some(),
        }
    }

    async fn screenshot(&self) -> Result<Frame> {
        match &self.screen {
            Some(s) => s.screenshot().await,
            None => Err(Error::setup(
                "the HID dongle gives input only; it has no screen",
                "Pair it with a screen source: HidDevice::with_screen(...) with an ap-ios device \
                 (DVT screenshots over USB) or an AirPlay mirroring receiver.",
            )),
        }
    }

    async fn tap(&self, at: Point) -> Result<()> {
        let (x, y) = xy(check_point(at)?);
        self.touch(|t| t.tap(x, y, TAP_HOLD)).await
    }

    async fn double_tap(&self, at: Point) -> Result<()> {
        let (x, y) = xy(check_point(at)?);
        self.touch(|t| {
            t.tap(x, y, TAP_HOLD);
            t.plan.wait(gesture::DOUBLE_TAP_GAP);
            t.tap(x, y, TAP_HOLD);
        })
        .await
    }

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()> {
        let (x, y) = xy(check_point(at)?);
        self.touch(|t| t.tap(x, y, duration)).await
    }

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let (from, to) = (xy(check_point(from)?), xy(check_point(to)?));
        self.touch(|t| t.swipe(from, to, duration, Duration::ZERO))
            .await
    }

    /// Drag the finger `amount` of the screen in `direction`, centred on `at`
    /// (upstream `scroll`: a 0.35 s swipe, kept inside 0.05..0.95).
    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()> {
        let at = check_point(at)?;
        if !(amount.is_finite() && amount > 0.0) {
            return Err(Error::InvalidArgument(format!("scroll amount {amount}")));
        }
        let (vx, vy) = direction.vector();
        let half = amount.min(0.9) / 2.0;
        let c = |v: f32| v.clamp(0.05, 0.95);
        let from = Point::new(c(at.x - vx * half), c(at.y - vy * half));
        let to = Point::new(c(at.x + vx * half), c(at.y + vy * half));
        self.swipe(from, to, SCROLL_DURATION).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.keys(|p| gesture::type_text(p, text)).await
    }

    async fn key(&self, key: &Key) -> Result<()> {
        let (code, mods) = key_usage(key)?;
        self.keys(|p| {
            gesture::key_press(p, code, mods);
            Ok(())
        })
        .await
    }

    async fn button(&self, button: Button) -> Result<()> {
        match button {
            Button::Home => {
                self.keys(|p| {
                    gesture::home(p);
                    Ok(())
                })
                .await
            }
            Button::AppSwitcher => self.system(SystemGesture::AppSwitcher).await,
            Button::Back => self.system(SystemGesture::Back).await,
            other => Err(Error::unsupported(format!(
                "button {other:?} over the HID dongle (the report map has no consumer \
                 power/volume/Siri usages; only Home, AppSwitcher and Back work)"
            ))),
        }
    }

    /// Cmd+H, Cmd+Space, type the app name, Enter. Bundle ids are mapped to
    /// names (see [`gesture::app_search_name`]).
    async fn launch_app(&self, bundle_or_package: &str) -> Result<()> {
        let name = gesture::app_search_name(bundle_or_package);
        if name.is_empty() {
            return Err(Error::InvalidArgument("empty app name".into()));
        }
        self.keys(|p| gesture::open_app(p, &name)).await
    }
}

/// [`DeviceProvider`] over serial ports.
#[derive(Debug, Clone, Default)]
pub struct HidProvider {
    /// Calibration used for connected devices.
    pub profile: CalibrationProfile,
}

impl HidProvider {
    pub fn new(profile: CalibrationProfile) -> Self {
        Self { profile }
    }
}

#[async_trait]
impl DeviceProvider for HidProvider {
    fn name(&self) -> &str {
        "hid"
    }

    /// Candidate ports (not opened, so not confirmed).
    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        let ports = tokio::task::spawn_blocking(discover)
            .await
            .map_err(|e| Error::Other(e.to_string()))??;
        Ok(ports
            .into_iter()
            .map(|c| DeviceInfo {
                id: format!("hid:{}", c.port),
                name: c
                    .product
                    .clone()
                    .or(c.board.map(String::from))
                    .unwrap_or_else(|| "serial HID dongle".into()),
                platform: Platform::Ios,
                os_version: String::new(),
                model: self.profile.models.first().cloned().unwrap_or_default(),
                screen_points: self.profile.screen_points,
                screen_pixels: Size::default(),
                backend: "hid".into(),
            })
            .collect())
    }

    /// `id` is `hid:COM5` or a bare port name.
    async fn connect(&self, id: &str) -> Result<Box<dyn Device>> {
        let port = id.strip_prefix("hid:").unwrap_or(id);
        Ok(Box::new(HidDevice::open(port, self.profile.clone()).await?))
    }
}
