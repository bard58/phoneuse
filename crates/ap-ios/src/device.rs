//! [`IosDevice`]: an ap-core [`Device`] backed by an [`IosBackend`].

use std::time::Duration;

use ap_core::device::check_point;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, Error, Frame, IosConfig, Key, Point, Result,
    SwipeDirection, async_trait,
};

use crate::backend::IosBackend;
use crate::cancel::CancelHandle;
use crate::coords::scroll_points;
use crate::hid::{
    HidBackend, TunnelSource, WORK_DVT, WORK_SHORT, job_deadline, work_gesture, work_text,
};
use crate::provider::{Resolved, resolve};
use crate::version::{BackendKind, IosVersion, select_backends};
use crate::worker::Worker;

/// Duration of a scroll gesture.
pub const SCROLL_DURATION: Duration = Duration::from_millis(350);

/// A connected iPhone.
pub struct IosDevice {
    info: DeviceInfo,
    caps: Capabilities,
    version: IosVersion,
    worker: Worker,
    /// The app opened by the last `launch_app`, until the next input.
    /// iOS gives no foreground-app query over this route.
    last_app: AppTracker,
}

/// Tracks the app opened by `launch_app` (see [`IosDevice::foreground_app`]).
///
/// Fail closed: every input (tap, swipe, scroll, text, key, button, pinch)
/// forgets the app, because any of them can open another app (a link, a
/// notification, a share sheet, an edge swipe, Enter on a link). So the app
/// is known only between a successful `launch_app` and the next input.
#[derive(Debug, Default)]
pub struct AppTracker(std::sync::Mutex<Option<String>>);

impl AppTracker {
    pub fn get(&self) -> Option<String> {
        self.0.lock().map(|g| g.clone()).unwrap_or(None)
    }

    pub fn set(&self, app: Option<String>) {
        if let Ok(mut g) = self.0.lock() {
            *g = app;
        }
    }

    /// Forget the app before an input.
    pub fn on_input(&self) {
        self.set(None);
    }
}

impl std::fmt::Debug for IosDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IosDevice")
            .field("info", &self.info)
            .finish()
    }
}

/// Build and connect one backend kind.
async fn open_backend(kind: BackendKind, r: &Resolved) -> Result<Box<dyn IosBackend>> {
    match kind {
        BackendKind::Hid => {
            let mut last = None;
            for source in r.sources() {
                let mut b = HidBackend::new(source.clone(), r.lockdown.clone());
                match Box::pin(b.connect()).await {
                    Ok(()) => return Ok(Box::new(b)),
                    Err(e) => {
                        tracing::info!("tunnel via {} failed: {e}", source_name(&source));
                        last = Some(e);
                    }
                }
            }
            Err(last.unwrap_or_else(|| Error::NotConnected("no route to the iPhone".into())))
        }
    }
}

/// Time limit for the screenshot that measures an unknown model's screen.
const MEASURE_TIMEOUT: Duration = Duration::from_secs(15);

/// Set the screen size of a model that is not in the model table from one
/// screenshot. If the screenshot fails, keep the default size and log a
/// warning. This never stops the connection: touch input uses normalized
/// coordinates and does not need the size.
async fn measure_unknown_screen(b: &mut dyn IosBackend, info: &mut DeviceInfo) {
    let shot = tokio::time::timeout(MEASURE_TIMEOUT, b.screenshot()).await;
    let measured = match shot {
        Ok(Ok(f)) => crate::models::sizes_from_screenshot(f.size()),
        Ok(Err(e)) => {
            tracing::warn!("screenshot for screen size failed: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("screenshot for screen size timed out");
            None
        }
    };
    match measured {
        Some((points, pixels)) => {
            tracing::info!(
                "unknown iPhone model {:?}: screen size {}x{} pt, {}x{} px, from a screenshot",
                info.model,
                points.width,
                points.height,
                pixels.width,
                pixels.height
            );
            info.screen_points = points;
            info.screen_pixels = pixels;
        }
        None => tracing::warn!(
            "unknown iPhone model {:?}: screen size not measured, using the default {}x{} pt",
            info.model,
            info.screen_points.width,
            info.screen_points.height
        ),
    }
}

fn source_name(s: &TunnelSource) -> &'static str {
    match s {
        TunnelSource::Lockdown(_) => "CoreDeviceProxy",
        TunnelSource::RemotePairing { .. } => "RemotePairing",
    }
}

impl IosDevice {
    /// Connect to the device with this UDID (or a unique UDID prefix).
    ///
    /// Errors: [`Error::Setup`] for iOS below 17.4, missing pairing, missing
    /// usbmuxd, missing DDI services. UNVERIFIED on a device.
    pub async fn connect(cfg: &IosConfig, id: &str) -> Result<IosDevice> {
        let cfg = cfg.clone();
        let id = id.to_string();
        let name = format!("ap-ios-{}", id.chars().take(12).collect::<String>());
        let (worker, (info, caps, version)) = Worker::start(name, move || async move {
            let r = Box::pin(resolve(&cfg, &id)).await?;
            let kinds = select_backends(r.version, &cfg.backend_order)?;
            let mut last = None;
            for kind in kinds {
                match Box::pin(open_backend(kind, &r)).await {
                    Ok(mut b) => {
                        let mut info = r.info.clone();
                        info.backend = b.name().to_string();
                        if crate::models::lookup(&info.model).is_none() {
                            Box::pin(measure_unknown_screen(&mut *b, &mut info)).await;
                        }
                        let caps = b.capabilities();
                        return Ok((b, (info, caps, r.version)));
                    }
                    Err(e) => {
                        tracing::warn!("iOS backend {} failed: {e}", kind.name());
                        last = Some(e);
                    }
                }
            }
            Err(last.unwrap_or_else(|| Error::NotConnected("no iOS backend".into())))
        })
        .await?;
        Ok(IosDevice {
            info,
            caps,
            version,
            worker,
            last_app: AppTracker::default(),
        })
    }

    /// The device's iOS version.
    pub fn ios_version(&self) -> IosVersion {
        self.version
    }

    /// A `Send + Sync` handle that cancels this device's in-flight calls
    /// (see [`CancelHandle`]). Keep it next to the `Box<dyn Device>` and call
    /// [`CancelHandle::cancel`] from another task (for example on Ctrl+C).
    /// The stopped call returns [`crate::cancelled`].
    pub fn cancel_handle(&self) -> CancelHandle {
        self.worker.cancel_handle()
    }

    /// Cancel every call submitted before now. Same as
    /// `self.cancel_handle().cancel()`.
    pub fn cancel(&self) {
        self.worker.cancel_handle().cancel();
    }

    /// Two-finger pinch around `center`. `from`/`to` are the finger
    /// distances (normalized). `from < to` zooms in.
    pub async fn pinch(&self, center: Point, from: f32, to: f32, duration: Duration) -> Result<()> {
        let center = check_point(center)?;
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(work_gesture(duration)), move |b| {
                b.pinch(center, from, to, duration)
            })
            .await
    }
}

#[async_trait]
impl Device for IosDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Stop every call sent before now (see [`IosDevice::cancel_handle`]).
    fn cancel(&self) {
        self.worker.cancel_handle().cancel();
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn screenshot(&self) -> Result<Frame> {
        self.worker
            .call_within(job_deadline(WORK_DVT), |b| b.screenshot())
            .await
    }

    async fn tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(WORK_SHORT), move |b| b.tap(at))
            .await
    }

    async fn double_tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(WORK_SHORT), move |b| b.double_tap(at))
            .await
    }

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()> {
        let at = check_point(at)?;
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(work_gesture(duration)), move |b| {
                b.long_press(at, duration)
            })
            .await
    }

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let (from, to) = (check_point(from)?, check_point(to)?);
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(work_gesture(duration)), move |b| {
                b.swipe(from, to, duration)
            })
            .await
    }

    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()> {
        let at = check_point(at)?;
        let (from, to) = scroll_points(at, direction, amount);
        self.last_app.on_input();
        self.worker
            .call_within(job_deadline(work_gesture(SCROLL_DURATION)), move |b| {
                b.swipe(from, to, SCROLL_DURATION)
            })
            .await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.last_app.on_input();
        let deadline = job_deadline(work_text(text.chars().count()));
        let text = text.to_string();
        self.worker
            .call_within(deadline, move |b| {
                Box::pin(async move { b.type_text(&text).await })
            })
            .await
    }

    async fn key(&self, key: &Key) -> Result<()> {
        self.last_app.on_input();
        let key = key.clone();
        self.worker
            .call_within(job_deadline(WORK_SHORT), move |b| {
                Box::pin(async move { b.key(&key).await })
            })
            .await
    }

    async fn button(&self, button: Button) -> Result<()> {
        self.last_app.on_input();
        // The longest press (Siri) holds 1.2 s. 5 s covers every sequence.
        self.worker
            .call_within(
                job_deadline(work_gesture(Duration::from_secs(5))),
                move |b| b.button(button),
            )
            .await
    }

    async fn launch_app(&self, bundle_id: &str) -> Result<()> {
        let id = bundle_id.to_string();
        // Unknown while the launch runs; known only after it succeeds.
        self.last_app.set(None);
        self.worker
            .call_within(job_deadline(WORK_DVT), move |b| {
                Box::pin(async move { b.launch_app(&id).await })
            })
            .await?;
        self.last_app.set(Some(bundle_id.to_string()));
        Ok(())
    }

    async fn list_apps(&self) -> Result<Vec<String>> {
        self.worker
            .call_within(job_deadline(WORK_DVT), |b| b.list_apps())
            .await
    }

    async fn ui_tree(&self) -> Result<Option<String>> {
        self.worker
            .call_within(job_deadline(WORK_DVT), |b| b.ui_tree())
            .await
    }

    /// The app opened by the last `launch_app`, until the next input. After
    /// any tap, swipe, scroll, text, key, button or pinch it is `None`
    /// (unknown), because iOS gives no foreground-app query over this route
    /// and any input can open another app. So on iPhone the app is known only
    /// on the first look after `launch_app`.
    async fn foreground_app(&self) -> Result<Option<String>> {
        Ok(self.last_app.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_tracker_forgets_on_input() {
        let t = AppTracker::default();
        assert_eq!(t.get(), None);
        t.set(Some("com.apple.Preferences".into()));
        assert_eq!(t.get().as_deref(), Some("com.apple.Preferences"));
        t.on_input();
        assert_eq!(t.get(), None);
    }

    /// Accepts every call.
    struct Null;

    #[async_trait::async_trait(?Send)]
    impl IosBackend for Null {
        fn name(&self) -> &'static str {
            "null"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::NONE
        }
        async fn screenshot(&mut self) -> Result<Frame> {
            Ok(Frame::solid(2, 2, [0, 0, 0, 255]))
        }
        async fn tap(&mut self, _: Point) -> Result<()> {
            Ok(())
        }
        async fn double_tap(&mut self, _: Point) -> Result<()> {
            Ok(())
        }
        async fn long_press(&mut self, _: Point, _: Duration) -> Result<()> {
            Ok(())
        }
        async fn swipe(&mut self, _: Point, _: Point, _: Duration) -> Result<()> {
            Ok(())
        }
        async fn type_text(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn key(&mut self, _: &Key) -> Result<()> {
            Ok(())
        }
        async fn button(&mut self, _: Button) -> Result<()> {
            Ok(())
        }
        async fn launch_app(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
    }

    async fn null_device() -> IosDevice {
        let (worker, ()) = Worker::start("ap-ios-null".into(), || async {
            let b: Box<dyn IosBackend> = Box::new(Null);
            Ok((b, ()))
        })
        .await
        .unwrap();
        IosDevice {
            info: DeviceInfo {
                id: "null".into(),
                name: "null".into(),
                platform: ap_core::Platform::Ios,
                os_version: "26.0".into(),
                model: "iPhone14,5".into(),
                screen_points: ap_core::Size::new(390.0, 844.0),
                screen_pixels: ap_core::Size::new(1170.0, 2532.0),
                backend: "null".into(),
            },
            caps: Capabilities::NONE,
            version: IosVersion::parse("26.0").unwrap(),
            worker,
            last_app: AppTracker::default(),
        }
    }

    const APP: &str = "com.apple.Preferences";

    /// An unknown model keeps the default size when the screenshot is not
    /// usable (Null gives 2x2 px), and the connection does not fail.
    #[tokio::test]
    async fn unknown_model_keeps_default_size_without_usable_screenshot() {
        let mut info = null_device().await.info.clone();
        info.model = "iPhone99,1".into();
        let mut b = Null;
        measure_unknown_screen(&mut b, &mut info).await;
        assert_eq!(info.screen_points, crate::models::FALLBACK_POINTS);
        assert_eq!(info.screen_pixels, crate::models::FALLBACK_PIXELS);
    }

    /// Every input makes the foreground app unknown, so the Jev guard
    /// (block_unknown_app) does not trust a stale bundle id.
    #[tokio::test]
    async fn foreground_app_is_unknown_after_any_input() {
        let d = null_device().await;
        let p = Point::new(0.5, 0.8);
        let ms = Duration::from_millis(10);
        for step in 0..9 {
            d.launch_app(APP).await.unwrap();
            assert_eq!(d.foreground_app().await.unwrap().as_deref(), Some(APP));
            // Observation does not forget the app.
            d.screenshot().await.unwrap();
            assert_eq!(d.foreground_app().await.unwrap().as_deref(), Some(APP));
            match step {
                0 => d.tap(p).await.unwrap(),
                1 => d.double_tap(p).await.unwrap(),
                2 => d.long_press(p, ms).await.unwrap(),
                3 => d.swipe(Point::new(0.5, 0.0), p, ms).await.unwrap(),
                // Starts at the bottom edge: the go-home gesture.
                4 => d.scroll(p, SwipeDirection::Up, 0.5).await.unwrap(),
                5 => d.type_text("hi").await.unwrap(),
                6 => d.key(&Key::Enter).await.unwrap(),
                7 => d.button(Button::VolumeUp).await.unwrap(),
                _ => {
                    let _ = d.pinch(Point::CENTER, 0.1, 0.3, ms).await;
                }
            }
            assert_eq!(d.foreground_app().await.unwrap(), None, "step {step}");
        }
    }

    #[tokio::test]
    async fn cancel_handle_does_not_block_later_calls() {
        let d = null_device().await;
        let h = d.cancel_handle();
        h.cancel();
        d.cancel();
        d.tap(Point::CENTER).await.unwrap();
    }
}
