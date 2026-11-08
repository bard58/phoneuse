//! [`IosDevice`]: an ap-core [`Device`] backed by an [`IosBackend`].

use std::time::Duration;

use ap_core::device::check_point;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, Error, Frame, IosConfig, Key, Point, Result,
    SwipeDirection, async_trait,
};

use crate::backend::IosBackend;
use crate::coords::scroll_points;
use crate::hid::{HidBackend, TunnelSource};
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
    /// The app opened by the last `launch_app`. Cleared by anything that can
    /// leave the app (Home, App Switcher, Lock, a swipe from the bottom edge,
    /// a Cmd shortcut). iOS gives no foreground-app query over this route.
    last_app: AppTracker,
}

/// Tracks the app opened by `launch_app` (see [`IosDevice::foreground_app`]).
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

    /// Forget the app when a button can leave it.
    pub fn on_button(&self, b: Button) {
        if matches!(b, Button::Home | Button::AppSwitcher | Button::Lock) {
            self.set(None);
        }
    }

    /// Forget the app on a swipe from the bottom edge (go home, switch apps).
    pub fn on_swipe(&self, from: Point) {
        if from.y >= 0.95 {
            self.set(None);
        }
    }

    /// Forget the app on a Cmd shortcut (Cmd+H, Cmd+Space, Cmd+Tab).
    pub fn on_key(&self, k: &Key) {
        if k.parts().0.meta {
            self.set(None);
        }
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
                    Ok(b) => {
                        let mut info = r.info.clone();
                        info.backend = b.name().to_string();
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

    /// Two-finger pinch around `center`. `from`/`to` are the finger
    /// distances (normalized). `from < to` zooms in.
    pub async fn pinch(&self, center: Point, from: f32, to: f32, duration: Duration) -> Result<()> {
        let center = check_point(center)?;
        self.worker
            .call(move |b| b.pinch(center, from, to, duration))
            .await
    }
}

#[async_trait]
impl Device for IosDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn screenshot(&self) -> Result<Frame> {
        self.worker.call(|b| b.screenshot()).await
    }

    async fn tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.worker.call(move |b| b.tap(at)).await
    }

    async fn double_tap(&self, at: Point) -> Result<()> {
        let at = check_point(at)?;
        self.worker.call(move |b| b.double_tap(at)).await
    }

    async fn long_press(&self, at: Point, duration: Duration) -> Result<()> {
        let at = check_point(at)?;
        self.worker.call(move |b| b.long_press(at, duration)).await
    }

    async fn swipe(&self, from: Point, to: Point, duration: Duration) -> Result<()> {
        let (from, to) = (check_point(from)?, check_point(to)?);
        self.last_app.on_swipe(from);
        self.worker.call(move |b| b.swipe(from, to, duration)).await
    }

    async fn scroll(&self, at: Point, direction: SwipeDirection, amount: f32) -> Result<()> {
        let at = check_point(at)?;
        let (from, to) = scroll_points(at, direction, amount);
        self.worker
            .call(move |b| b.swipe(from, to, SCROLL_DURATION))
            .await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let text = text.to_string();
        self.worker
            .call(move |b| Box::pin(async move { b.type_text(&text).await }))
            .await
    }

    async fn key(&self, key: &Key) -> Result<()> {
        self.last_app.on_key(key);
        let key = key.clone();
        self.worker
            .call(move |b| Box::pin(async move { b.key(&key).await }))
            .await
    }

    async fn button(&self, button: Button) -> Result<()> {
        self.last_app.on_button(button);
        self.worker.call(move |b| b.button(button)).await
    }

    async fn launch_app(&self, bundle_id: &str) -> Result<()> {
        let id = bundle_id.to_string();
        // Unknown while the launch runs; known only after it succeeds.
        self.last_app.set(None);
        self.worker
            .call(move |b| Box::pin(async move { b.launch_app(&id).await }))
            .await?;
        self.last_app.set(Some(bundle_id.to_string()));
        Ok(())
    }

    async fn list_apps(&self) -> Result<Vec<String>> {
        self.worker.call(|b| b.list_apps()).await
    }

    async fn ui_tree(&self) -> Result<Option<String>> {
        self.worker.call(|b| b.ui_tree()).await
    }

    /// The app opened by the last `launch_app`, or `None` (unknown) after
    /// anything that can leave it. A tap that opens another app (a link, a
    /// share sheet) is not detected.
    async fn foreground_app(&self) -> Result<Option<String>> {
        Ok(self.last_app.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ap_core::Modifiers;

    #[test]
    fn app_tracker_forgets_on_leave() {
        let t = AppTracker::default();
        assert_eq!(t.get(), None);
        t.set(Some("com.apple.Preferences".into()));
        t.on_button(Button::VolumeUp);
        t.on_swipe(Point::new(0.5, 0.5));
        t.on_key(&Key::Enter);
        assert_eq!(t.get().as_deref(), Some("com.apple.Preferences"));
        t.on_swipe(Point::new(0.5, 0.99));
        assert_eq!(t.get(), None);
        t.set(Some("a.b".into()));
        t.on_button(Button::Home);
        assert_eq!(t.get(), None);
        t.set(Some("a.b".into()));
        let cmd = Modifiers {
            meta: true,
            ..Modifiers::NONE
        };
        t.on_key(&Key::Char('h').with(cmd));
        assert_eq!(t.get(), None);
    }
}
